//! Registro y agregación de latencias con histogramas de alta resolución.

use hdrhistogram::errors::{CreationError, RecordError};
use hdrhistogram::sync::{Recorder, SyncHistogram};
use hdrhistogram::Histogram;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

/// Latencia mínima representable en nanosegundos (HdrHistogram exige >= 1).
const MIN_LATENCY_NS: u64 = 1;

/// Latencia máxima representable: 10 segundos.
const MAX_LATENCY_NS: u64 = 10_000_000_000;

/// Precisión del histograma (dígitos significativos).
const SIGNIFICANT_DIGITS: u8 = 3;

/// Errores del módulo de métricas.
#[derive(Debug, Error)]
pub enum MetricsError {
    /// Fallo al crear el histograma subyacente.
    #[error("failed to create latency histogram: {0}")]
    HistogramCreation(#[from] CreationError),

    /// Fallo al registrar una muestra de latencia.
    #[error("failed to record latency sample: {0}")]
    Record(#[from] RecordError),
}

/// Instantánea de percentiles de latencia calculada sobre las muestras registradas.
#[derive(Debug, Clone, PartialEq)]
pub struct LatencySnapshot {
    /// Número total de muestras incorporadas al histograma.
    pub count: u64,
    /// Percentil 50 (mediana), en nanosegundos.
    pub p50_ns: u64,
    /// Percentil 90, en nanosegundos.
    pub p90_ns: u64,
    /// Percentil 99, en nanosegundos.
    pub p99_ns: u64,
    /// Percentil 99.9, en nanosegundos.
    pub p999_ns: u64,
    /// Latencia mínima observada, en nanosegundos.
    pub min_ns: u64,
    /// Latencia máxima observada, en nanosegundos.
    pub max_ns: u64,
    /// Latencia media aritmética, en nanosegundos.
    pub mean_ns: f64,
}

impl LatencySnapshot {
    fn empty() -> Self {
        Self {
            count: 0,
            p50_ns: 0,
            p90_ns: 0,
            p99_ns: 0,
            p999_ns: 0,
            min_ns: 0,
            max_ns: 0,
            mean_ns: 0.0,
        }
    }
}

impl fmt::Display for LatencySnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Latency snapshot ({} samples)", self.count)?;
        writeln!(
            f,
            "  p50={} ns  p90={} ns  p99={} ns  p99.9={} ns",
            self.p50_ns, self.p90_ns, self.p99_ns, self.p999_ns
        )?;
        write!(
            f,
            "  min={} ns  max={} ns  mean={:.1} ns",
            self.min_ns, self.max_ns, self.mean_ns
        )
    }
}

/// Registra latencias y calcula percentiles (p50, p90, p99, p99.9).
///
/// Internamente usa [`SyncHistogram`] de `hdrhistogram` para agregación thread-safe:
/// cada hilo obtiene un [`Recorder`] independiente y consolida con [`LatencyRecorder::snapshot`].
pub struct LatencyRecorder {
    histogram: Arc<SyncHistogramWrapper>,
}

struct SyncHistogramWrapper {
    inner: std::sync::Mutex<SyncHistogram<u64>>,
}

impl LatencyRecorder {
    /// Crea un recorder con rango [1 ns, 10 s] y precisión de 3 dígitos significativos.
    ///
    /// # Returns
    ///
    /// Un recorder listo para uso concurrente, o [`MetricsError`] si el histograma no puede crearse.
    pub fn new() -> Result<Self, MetricsError> {
        let base = Histogram::<u64>::new_with_bounds(
            MIN_LATENCY_NS,
            MAX_LATENCY_NS,
            SIGNIFICANT_DIGITS,
        )?;
        let sync: SyncHistogram<u64> = base.into();

        Ok(Self {
            histogram: Arc::new(SyncHistogramWrapper {
                inner: std::sync::Mutex::new(sync),
            }),
        })
    }

    /// Obtiene un handle de escritura para el hilo actual.
    ///
    /// # Returns
    ///
    /// Un [`Recorder`] wait-free; las muestras se consolidan al llamar [`Self::snapshot`].
    pub fn recorder(&self) -> Recorder<u64> {
        let guard = self
            .histogram
            .inner
            .lock()
            .expect("latency histogram mutex poisoned");
        guard.recorder()
    }

    /// Registra una latencia en nanosegundos usando el recorder del hilo actual.
    ///
    /// # Inputs
    ///
    /// - `writer`: handle obtenido con [`Self::recorder`].
    /// - `latency_ns`: duración en nanosegundos (se satura al rango representable).
    ///
    /// # Returns
    ///
    /// `Ok(())` tras registrar la muestra, o [`MetricsError::Record`] si el valor es inválido.
    pub fn record_ns(writer: &mut Recorder<u64>, latency_ns: u64) -> Result<(), MetricsError> {
        let clamped = latency_ns.clamp(MIN_LATENCY_NS, MAX_LATENCY_NS);
        writer.record(clamped)?;
        Ok(())
    }

    /// Registra una latencia medida como [`Duration`].
    ///
    /// # Inputs
    ///
    /// - `writer`: handle obtenido con [`Self::recorder`].
    /// - `latency`: duración medida (p. ej. de un `Instant::elapsed()`).
    ///
    /// # Returns
    ///
    /// `Ok(())` tras registrar la muestra, o [`MetricsError::Record`] en caso de error.
    pub fn record_duration(
        writer: &mut Recorder<u64>,
        latency: Duration,
    ) -> Result<(), MetricsError> {
        let ns = u64::try_from(latency.as_nanos()).unwrap_or(MAX_LATENCY_NS);
        Self::record_ns(writer, ns)
    }

    /// Consolida muestras pendientes y devuelve los percentiles calculados.
    ///
    /// # Returns
    ///
    /// Una [`LatencySnapshot`] con p50, p90, p99 y p99.9. Si no hay muestras, todos los valores son cero.
    pub fn snapshot(&self) -> LatencySnapshot {
        let mut guard = self
            .histogram
            .inner
            .lock()
            .expect("latency histogram mutex poisoned");

        guard.refresh();

        let count = guard.len();
        if count == 0 {
            return LatencySnapshot::empty();
        }

        LatencySnapshot {
            count,
            p50_ns: guard.value_at_quantile(0.50),
            p90_ns: guard.value_at_quantile(0.90),
            p99_ns: guard.value_at_quantile(0.99),
            p999_ns: guard.value_at_quantile(0.999),
            min_ns: guard.min(),
            max_ns: guard.max(),
            mean_ns: guard.mean(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    /// Comprueba que un snapshot vacío devuelve conteo cero.
    #[test]
    fn empty_recorder_returns_zero_snapshot() {
        let recorder = LatencyRecorder::new().expect("recorder should initialize");
        let snap = recorder.snapshot();
        assert_eq!(snap, LatencySnapshot::empty());
    }

    /// Comprueba percentiles sobre una distribución uniforme 1..=1000.
    #[test]
    fn computes_expected_percentiles_for_uniform_distribution() {
        let recorder = LatencyRecorder::new().expect("recorder should initialize");
        let mut writer = recorder.recorder();

        for value in 1..=1_000 {
            LatencyRecorder::record_ns(&mut writer, value).expect("sample should be in range");
        }
        drop(writer);

        let snap = recorder.snapshot();
        assert_eq!(snap.count, 1_000);
        assert_eq!(snap.min_ns, 1);
        assert_eq!(snap.max_ns, 1_000);

        assert_within(snap.p50_ns, 500, 5, "p50");
        assert_within(snap.p90_ns, 900, 5, "p90");
        assert_within(snap.p99_ns, 990, 5, "p99");
        assert_within(snap.p999_ns, 999, 5, "p99.9");
    }

    /// Comprueba registro vía `Duration`.
    #[test]
    fn records_duration_samples() {
        let recorder = LatencyRecorder::new().expect("recorder should initialize");
        let mut writer = recorder.recorder();

        LatencyRecorder::record_duration(&mut writer, Duration::from_micros(500))
            .expect("duration sample should record");
        drop(writer);

        let snap = recorder.snapshot();
        assert_eq!(snap.count, 1);
        assert_within(snap.p50_ns, 500_000, 10_000, "p50 from duration");
    }

    /// Comprueba agregación thread-safe desde múltiples hilos.
    #[test]
    fn aggregates_samples_from_multiple_threads() {
        let recorder = Arc::new(LatencyRecorder::new().expect("recorder should initialize"));
        let thread_count = 4;
        let samples_per_thread = 250;

        let handles: Vec<_> = (0..thread_count)
            .map(|thread_id| {
                let recorder = Arc::clone(&recorder);
                thread::spawn(move || {
                    let mut writer = recorder.recorder();
                    let base = thread_id * samples_per_thread;
                    for offset in 1..=samples_per_thread {
                        let value = (base + offset) as u64;
                        LatencyRecorder::record_ns(&mut writer, value)
                            .expect("concurrent sample should record");
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().expect("worker thread should not panic");
        }

        let snap = recorder.snapshot();
        assert_eq!(snap.count, (thread_count * samples_per_thread) as u64);
        assert_eq!(snap.min_ns, 1);
        assert_eq!(snap.max_ns, 1_000);
        assert_within(snap.p50_ns, 500, 5, "concurrent p50");
    }

    fn assert_within(actual: u64, expected: u64, tolerance: u64, label: &str) {
        assert!(
            actual.abs_diff(expected) <= tolerance,
            "{label}: expected ~{expected} ± {tolerance}, got {actual}"
        );
    }
}
