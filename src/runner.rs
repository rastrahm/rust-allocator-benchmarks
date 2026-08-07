//! Orquestación del benchmark concurrente: productor, workers y agregación de métricas.

use crate::metrics::{LatencyRecorder, LatencySnapshot, MetricsError};
use crate::workload::{process_and_record, PacketFormat, WorkloadConfig, WorkloadError};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use thiserror::Error;

/// Configuración de una ejecución del benchmark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchmarkConfig {
    /// Número de hilos worker que consumen payloads del canal.
    pub threads: usize,
    /// Cantidad total de payloads a procesar.
    pub iterations: u64,
    /// Tamaño de cada payload simulado (bytes).
    pub payload_bytes: usize,
    /// Formato de parsing simulado.
    pub format: PacketFormat,
    /// Semilla base; cada iteración usa `seed + iteration_index`.
    pub seed: u64,
    /// Capacidad del canal acotado (backpressure productor/consumidor).
    pub queue_depth: usize,
}

impl Default for BenchmarkConfig {
    fn default() -> Self {
        let threads = thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(4);

        Self {
            threads,
            iterations: 10_000,
            payload_bytes: 2_048,
            format: PacketFormat::JsonLike,
            seed: 0,
            queue_depth: threads * 4,
        }
    }
}

/// Resultado agregado de una ejecución del benchmark.
#[derive(Debug, Clone, PartialEq)]
pub struct BenchmarkResult {
    /// Percentiles de latencia consolidados.
    pub snapshot: LatencySnapshot,
    /// Tiempo total de wall-clock de la ejecución.
    pub elapsed: Duration,
    /// Throughput en payloads por segundo.
    pub throughput_per_sec: f64,
}

/// Errores al ejecutar el benchmark.
#[derive(Debug, Error)]
pub enum BenchmarkError {
    /// Configuración inválida.
    #[error("{0}")]
    InvalidConfig(String),

    /// Error del workload o métricas.
    #[error(transparent)]
    Workload(#[from] WorkloadError),

    /// Error al inicializar métricas.
    #[error(transparent)]
    Metrics(#[from] MetricsError),

    /// Un worker terminó con panic.
    #[error("benchmark worker thread panicked")]
    WorkerPanicked,
}

/// Ejecuta el benchmark concurrente con pipeline productor/consumidor.
///
/// # Inputs
///
/// - `config`: hilos, iteraciones, tamaño de payload y profundidad de cola.
///
/// # Returns
///
/// [`BenchmarkResult`] con percentiles y throughput, o un error de configuración/ejecución.
pub fn run_benchmark(config: &BenchmarkConfig) -> Result<BenchmarkResult, BenchmarkError> {
    validate_config(config)?;

    let (job_tx, job_rx) = bounded::<WorkloadConfig>(config.queue_depth);
    let recorder = Arc::new(LatencyRecorder::new()?);

    let started = Instant::now();
    let producer = spawn_producer(job_tx, config);
    let workers = spawn_workers(config.threads, Arc::clone(&recorder), job_rx);

    producer.join().map_err(|_| BenchmarkError::WorkerPanicked)??;
    join_workers(workers)?;

    let elapsed = started.elapsed();
    let snapshot = recorder.snapshot();
    let throughput_per_sec = if elapsed.is_zero() {
        0.0
    } else {
        config.iterations as f64 / elapsed.as_secs_f64()
    };

    Ok(BenchmarkResult {
        snapshot,
        elapsed,
        throughput_per_sec,
    })
}

/// Valida parámetros de ejecución antes de lanzar hilos.
fn validate_config(config: &BenchmarkConfig) -> Result<(), BenchmarkError> {
    if config.threads == 0 {
        return Err(BenchmarkError::InvalidConfig(
            "threads must be at least 1".into(),
        ));
    }
    if config.iterations == 0 {
        return Err(BenchmarkError::InvalidConfig(
            "iterations must be at least 1".into(),
        ));
    }
    if config.queue_depth == 0 {
        return Err(BenchmarkError::InvalidConfig(
            "queue-depth must be at least 1".into(),
        ));
    }

    WorkloadConfig::new(config.payload_bytes, config.format, config.seed)?;
    Ok(())
}

/// Hilo productor: encola configs de workload con semilla por iteración.
fn spawn_producer(
    job_tx: Sender<WorkloadConfig>,
    config: &BenchmarkConfig,
) -> thread::JoinHandle<Result<(), BenchmarkError>> {
    let payload_bytes = config.payload_bytes;
    let format = config.format;
    let seed = config.seed;
    let iterations = config.iterations;

    thread::spawn(move || {
        for iteration in 0..iterations {
            let workload = WorkloadConfig::new(payload_bytes, format, seed.wrapping_add(iteration))?;
            job_tx
                .send(workload)
                .map_err(|_| BenchmarkError::InvalidConfig("job channel closed".into()))?;
        }
        Ok(())
    })
}

/// Lanza hilos worker que consumen configs del canal compartido.
fn spawn_workers(
    thread_count: usize,
    recorder: Arc<LatencyRecorder>,
    job_rx: Receiver<WorkloadConfig>,
) -> Vec<thread::JoinHandle<Result<(), WorkloadError>>> {
    (0..thread_count)
        .map(|_| {
            let recorder = Arc::clone(&recorder);
            let job_rx = job_rx.clone();

            thread::spawn(move || {
                let mut writer = recorder.recorder();
                for job in job_rx.iter() {
                    process_and_record(&job, &mut writer)?;
                }
                Ok(())
            })
        })
        .collect()
}

fn join_workers(
    workers: Vec<thread::JoinHandle<Result<(), WorkloadError>>>,
) -> Result<(), BenchmarkError> {
    for worker in workers {
        worker
            .join()
            .map_err(|_| BenchmarkError::WorkerPanicked)??;
    }
    Ok(())
}

/// Formatea nanosegundos como cadena legible (ns, µs o ms).
pub fn format_latency_ns(ns: u64) -> String {
    if ns >= 1_000_000 {
        format!("{:.2} ms", ns as f64 / 1_000_000.0)
    } else if ns >= 1_000 {
        format!("{:.2} µs", ns as f64 / 1_000.0)
    } else {
        format!("{ns} ns")
    }
}

/// Imprime una tabla de resultados del benchmark en stdout.
pub fn print_results(
    allocator: &str,
    config: &BenchmarkConfig,
    result: &BenchmarkResult,
) {
    let snap = &result.snapshot;
    println!();
    println!("Allocator Benchmark Results");
    println!("===========================");
    println!("Allocator:     {allocator}");
    println!("Threads:       {}", config.threads);
    println!("Iterations:    {}", config.iterations);
    println!("Payload size:  {} bytes", config.payload_bytes);
    println!("Format:        {:?}", config.format);
    println!("Queue depth:   {}", config.queue_depth);
    println!("Elapsed:       {:.3} s", result.elapsed.as_secs_f64());
    println!(
        "Throughput:    {:.0} payloads/s",
        result.throughput_per_sec
    );
    println!();
    println!("Latency percentiles:");
    println!(
        "  {:>6}  {:>12}",
        "Metric", "Value"
    );
    println!(
        "  {:>6}  {:>12}",
        "p50", format_latency_ns(snap.p50_ns)
    );
    println!(
        "  {:>6}  {:>12}",
        "p90", format_latency_ns(snap.p90_ns)
    );
    println!(
        "  {:>6}  {:>12}",
        "p99", format_latency_ns(snap.p99_ns)
    );
    println!(
        "  {:>6}  {:>12}",
        "p99.9", format_latency_ns(snap.p999_ns)
    );
    println!(
        "  {:>6}  {:>12}",
        "min", format_latency_ns(snap.min_ns)
    );
    println!(
        "  {:>6}  {:>12}",
        "max", format_latency_ns(snap.max_ns)
    );
    println!(
        "  {:>6}  {:>12}",
        "mean", format_latency_ns(snap.mean_ns.round() as u64)
    );
    println!("  samples: {}", snap.count);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zero_threads() {
        let config = BenchmarkConfig {
            threads: 0,
            ..BenchmarkConfig::default()
        };
        let err = run_benchmark(&config).unwrap_err();
        assert!(matches!(err, BenchmarkError::InvalidConfig(_)));
    }

    #[test]
    fn rejects_zero_iterations() {
        let config = BenchmarkConfig {
            iterations: 0,
            ..BenchmarkConfig::default()
        };
        let err = run_benchmark(&config).unwrap_err();
        assert!(matches!(err, BenchmarkError::InvalidConfig(_)));
    }

    #[test]
    fn run_benchmark_collects_all_samples() {
        let config = BenchmarkConfig {
            threads: 2,
            iterations: 200,
            payload_bytes: 512,
            queue_depth: 8,
            ..BenchmarkConfig::default()
        };

        let result = run_benchmark(&config).expect("benchmark should succeed");
        assert_eq!(result.snapshot.count, 200);
        assert!(result.throughput_per_sec > 0.0);
        assert!(result.snapshot.p99_ns >= result.snapshot.p50_ns);
    }

    #[test]
    fn format_latency_scales_units() {
        assert!(format_latency_ns(500).ends_with("ns"));
        assert!(format_latency_ns(5_000).contains('µ'));
        assert!(format_latency_ns(5_000_000).contains("ms"));
    }
}
