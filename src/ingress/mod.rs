//! Fuentes de ingesta de payloads: in-memory, async TCP e io_uring.

mod dispatch;
mod memory;
mod wire;

#[cfg(feature = "ingress-async")]
mod async_tcp;

#[cfg(feature = "ingress-io-uring")]
mod io_uring;

use crate::runner::BenchmarkError;
use crate::workload::WorkloadConfig;
use crossbeam_channel::Sender;
use std::thread::JoinHandle;

pub use memory::spawn_memory_producer;
#[cfg(feature = "ingress-async")]
pub use async_tcp::spawn_async_tcp_producer;
#[cfg(feature = "ingress-io-uring")]
pub use io_uring::spawn_io_uring_producer;

/// Modo de ingesta de red seleccionado en runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum IngressMode {
    /// Canal crossbeam in-process (comportamiento original).
    #[default]
    InMemory,
    /// Red async multi-conexión TCP (requiere feature `ingress-async`).
    AsyncTcp,
    /// Ingesta production-like con io_uring (requiere feature `ingress-io-uring`).
    IoUring,
}

/// Parámetros de configuración del subsistema de ingesta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngressConfig {
    /// Modo de transporte activo.
    pub mode: IngressMode,
    /// Puerto TCP para modos de red (async / io_uring).
    pub bind_port: u16,
    /// Número de conexiones cliente concurrentes hacia el listener.
    pub connections: usize,
    /// Tamaño máximo de batch de lectura por conexión.
    pub read_batch_size: usize,
}

impl Default for IngressConfig {
    fn default() -> Self {
        Self {
            mode: IngressMode::InMemory,
            bind_port: 9_876,
            connections: 4,
            read_batch_size: 64,
        }
    }
}

impl IngressConfig {
    /// Valida la configuración de ingesta según modos y features compiladas.
    ///
    /// # Returns
    ///
    /// `Ok(())` si la configuración es válida, o [`BenchmarkError::InvalidConfig`].
    pub fn validate(&self) -> Result<(), BenchmarkError> {
        match self.mode {
            IngressMode::InMemory => Ok(()),
            IngressMode::AsyncTcp => {
                if !cfg!(feature = "ingress-async") {
                    return Err(BenchmarkError::InvalidConfig(
                        "ingress mode async-tcp requires --features ingress-async".into(),
                    ));
                }
                validate_network_params(self)
            }
            IngressMode::IoUring => {
                if !cfg!(feature = "ingress-io-uring") {
                    return Err(BenchmarkError::InvalidConfig(
                        "ingress mode io-uring requires --features ingress-io-uring".into(),
                    ));
                }
                validate_network_params(self)
            }
        }
    }
}

fn validate_network_params(config: &IngressConfig) -> Result<(), BenchmarkError> {
    if config.connections == 0 {
        return Err(BenchmarkError::InvalidConfig(
            "ingress connections must be at least 1".into(),
        ));
    }
    if config.read_batch_size == 0 {
        return Err(BenchmarkError::InvalidConfig(
            "ingress read-batch-size must be at least 1".into(),
        ));
    }
    Ok(())
}

/// Handle del productor de jobs según el modo de ingesta activo.
pub enum IngressProducer {
    /// Productor in-memory en hilo std.
    Memory(JoinHandle<Result<(), BenchmarkError>>),
    /// Productor async TCP con runtime Tokio dedicado.
    #[cfg(feature = "ingress-async")]
    AsyncTcp(JoinHandle<Result<(), BenchmarkError>>),
    /// Productor io_uring con tokio-uring dedicado.
    #[cfg(feature = "ingress-io-uring")]
    IoUring(JoinHandle<Result<(), BenchmarkError>>),
}

/// Inicia el productor de payloads según [`IngressConfig`].
///
/// # Inputs
///
/// - `config`: parámetros de ingesta.
/// - `iterations`: cantidad total de payloads.
/// - `payload_bytes`, `format`, `seed`: plantilla de workload por iteración.
/// - `job_tx`: emisor del canal hacia los workers.
///
/// # Returns
///
/// Un [`IngressProducer`] listo para join, o error de configuración.
pub fn spawn_ingress_producer(
    config: &IngressConfig,
    iterations: u64,
    payload_bytes: usize,
    format: crate::workload::PacketFormat,
    seed: u64,
    job_tx: Sender<WorkloadConfig>,
) -> Result<IngressProducer, BenchmarkError> {
    config.validate()?;

    match config.mode {
        IngressMode::InMemory => {
            let handle = spawn_memory_producer(job_tx, iterations, payload_bytes, format, seed);
            Ok(IngressProducer::Memory(handle))
        }
        IngressMode::AsyncTcp => {
            #[cfg(feature = "ingress-async")]
            {
                let handle = spawn_async_tcp_producer(
                    config,
                    iterations,
                    payload_bytes,
                    format,
                    seed,
                    job_tx,
                );
                Ok(IngressProducer::AsyncTcp(handle))
            }
            #[cfg(not(feature = "ingress-async"))]
            {
                let _ = (config, iterations, payload_bytes, format, seed, job_tx);
                Err(BenchmarkError::InvalidConfig(
                    "ingress mode async-tcp requires --features ingress-async".into(),
                ))
            }
        }
        IngressMode::IoUring => {
            #[cfg(feature = "ingress-io-uring")]
            {
                let handle = spawn_io_uring_producer(
                    config,
                    iterations,
                    payload_bytes,
                    format,
                    seed,
                    job_tx,
                );
                Ok(IngressProducer::IoUring(handle))
            }
            #[cfg(not(feature = "ingress-io-uring"))]
            {
                let _ = (config, iterations, payload_bytes, format, seed, job_tx);
                Err(BenchmarkError::InvalidConfig(
                    "ingress mode io-uring requires --features ingress-io-uring".into(),
                ))
            }
        }
    }
}

/// Espera a que el productor de ingesta termine.
///
/// # Returns
///
/// `Ok(())` si el productor terminó correctamente, o un error de ejecución.
pub fn join_ingress_producer(producer: IngressProducer) -> Result<(), BenchmarkError> {
    match producer {
        IngressProducer::Memory(handle) => handle
            .join()
            .map_err(|_| BenchmarkError::WorkerPanicked)?,
        #[cfg(feature = "ingress-async")]
        IngressProducer::AsyncTcp(handle) => handle
            .join()
            .map_err(|_| BenchmarkError::WorkerPanicked)?,
        #[cfg(feature = "ingress-io-uring")]
        IngressProducer::IoUring(handle) => handle
            .join()
            .map_err(|_| BenchmarkError::WorkerPanicked)?,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workload::PacketFormat;
    use crossbeam_channel::bounded;

    #[test]
    fn default_ingress_is_in_memory() {
        assert_eq!(IngressConfig::default().mode, IngressMode::InMemory);
    }

    #[cfg(not(feature = "ingress-async"))]
    #[test]
    fn async_tcp_without_feature_is_rejected() {
        let config = IngressConfig {
            mode: IngressMode::AsyncTcp,
            ..IngressConfig::default()
        };
        let err = config.validate().unwrap_err();
        assert!(matches!(err, BenchmarkError::InvalidConfig(_)));
    }

    #[cfg(feature = "ingress-async")]
    #[test]
    fn async_tcp_producer_enqueues_all_jobs() {
        use crossbeam_channel::bounded;

        let config = IngressConfig {
            mode: IngressMode::AsyncTcp,
            bind_port: 19_876,
            connections: 2,
            read_batch_size: 8,
        };
        let (tx, rx) = bounded(64);
        let handle = spawn_async_tcp_producer(&config, 40, 256, PacketFormat::JsonLike, 0, tx);
        handle.join().unwrap().expect("async producer ok");
        for _ in 0..40 {
            assert!(rx.recv().is_ok());
        }
        assert!(rx.try_recv().is_err());
    }

    #[cfg(not(feature = "ingress-io-uring"))]
    #[test]
    fn io_uring_without_feature_is_rejected() {
        let config = IngressConfig {
            mode: IngressMode::IoUring,
            ..IngressConfig::default()
        };
        let err = config.validate().unwrap_err();
        assert!(matches!(err, BenchmarkError::InvalidConfig(_)));
    }

    #[cfg(feature = "ingress-io-uring")]
    #[test]
    fn io_uring_producer_enqueues_all_jobs() {
        use crossbeam_channel::bounded;

        let config = IngressConfig {
            mode: IngressMode::IoUring,
            bind_port: 39_876,
            connections: 2,
            read_batch_size: 8,
        };
        let (tx, rx) = bounded(64);
        let handle = spawn_io_uring_producer(&config, 40, 256, PacketFormat::JsonLike, 0, tx);
        handle.join().unwrap().expect("io-uring producer ok");
        for _ in 0..40 {
            assert!(rx.recv().is_ok());
        }
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn memory_producer_enqueues_all_jobs() {
        let (tx, rx) = bounded(50);
        let handle = spawn_memory_producer(tx, 50, 256, PacketFormat::JsonLike, 0);
        handle.join().unwrap().expect("producer ok");
        for _ in 0..50 {
            assert!(rx.recv().is_ok());
        }
        assert!(rx.try_recv().is_err());
    }
}
