//! Aceleradores opcionales: hooks post-procesamiento CPU/GPU.

mod none;

#[cfg(feature = "gpu-cuda")]
mod cuda_common;

#[cfg(feature = "gpu-cuda")]
mod cuda_kernels;

#[cfg(feature = "gpu-serious")]
mod cuda_serious;

use crate::runner::BenchmarkError;
use crate::workload::WorkloadConfig;
use hdrhistogram::sync::Recorder;
use std::sync::Arc;
use std::time::Duration;

pub use none::NoAccelerator;
#[cfg(feature = "gpu-cuda")]
pub use cuda_common::cuda_device_available;
#[cfg(feature = "gpu-cuda")]
pub use cuda_kernels::CudaKernelAccelerator;
#[cfg(feature = "gpu-serious")]
pub use cuda_serious::CudaSeriousAccelerator;

/// Modo de aceleración GPU seleccionado en runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum AcceleratorMode {
    /// Sin aceleración (comportamiento original).
    #[default]
    None,
    /// Kernels CUDA + alloc VRAM (requiere feature `gpu-cuda`).
    CudaKernels,
    /// Benchmark GPU sostenido multi-kernel (requiere feature `gpu-serious`).
    CudaSerious,
}

/// Configuración del subsistema de aceleración.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceleratorConfig {
    /// Modo activo.
    pub mode: AcceleratorMode,
    /// Índice de dispositivo CUDA (modos GPU).
    pub cuda_device: u32,
    /// Elementos por batch en kernels GPU.
    pub gpu_batch_size: usize,
}

impl Default for AcceleratorConfig {
    fn default() -> Self {
        Self {
            mode: AcceleratorMode::None,
            cuda_device: 0,
            gpu_batch_size: 256,
        }
    }
}

impl AcceleratorConfig {
    /// Valida el modo de aceleración según features compiladas.
    ///
    /// # Returns
    ///
    /// `Ok(())` o [`BenchmarkError::InvalidConfig`].
    pub fn validate(&self) -> Result<(), BenchmarkError> {
        match self.mode {
            AcceleratorMode::None => Ok(()),
            AcceleratorMode::CudaKernels => {
                if !cfg!(feature = "gpu-cuda") {
                    return Err(BenchmarkError::InvalidConfig(
                        "accelerator cuda-kernels requires --features gpu-cuda".into(),
                    ));
                }
                validate_gpu_params(self)
            }
            AcceleratorMode::CudaSerious => {
                if !cfg!(feature = "gpu-serious") {
                    return Err(BenchmarkError::InvalidConfig(
                        "accelerator cuda-serious requires --features gpu-serious".into(),
                    ));
                }
                if !cfg!(feature = "gpu-cuda") {
                    return Err(BenchmarkError::InvalidConfig(
                        "gpu-serious requires gpu-cuda feature".into(),
                    ));
                }
                validate_gpu_params(self)
            }
        }
    }
}

fn validate_gpu_params(config: &AcceleratorConfig) -> Result<(), BenchmarkError> {
    if config.gpu_batch_size == 0 {
        return Err(BenchmarkError::InvalidConfig(
            "gpu-batch-size must be at least 1".into(),
        ));
    }
    Ok(())
}

/// Hook ejecutado tras procesar cada payload.
pub trait AcceleratorHook: Send + Sync {
    /// Ejecuta trabajo acelerador opcional tras el parseo del payload.
    ///
    /// # Inputs
    ///
    /// - `job`: configuración del payload procesado.
    /// - `parse_latency`: latencia del parseo CPU-only.
    ///
    /// # Returns
    ///
    /// Latencia adicional del acelerador, o error.
    fn post_process(
        &self,
        job: &WorkloadConfig,
        parse_latency: Duration,
    ) -> Result<Duration, BenchmarkError>;
}

/// Construye el hook de aceleración según configuración.
///
/// # Returns
///
/// Implementación concreta del hook, o error si el modo no está disponible.
pub fn build_accelerator(
    config: &AcceleratorConfig,
) -> Result<Arc<dyn AcceleratorHook + Send + Sync>, BenchmarkError> {
    config.validate()?;

    match config.mode {
        AcceleratorMode::None => Ok(Arc::new(NoAccelerator)),
        AcceleratorMode::CudaKernels => {
            #[cfg(feature = "gpu-cuda")]
            {
                Ok(Arc::new(CudaKernelAccelerator::new(config)?))
            }
            #[cfg(not(feature = "gpu-cuda"))]
            {
                unreachable!("validated above")
            }
        }
        AcceleratorMode::CudaSerious => {
            #[cfg(feature = "gpu-serious")]
            {
                Ok(Arc::new(CudaSeriousAccelerator::new(config)?))
            }
            #[cfg(not(feature = "gpu-serious"))]
            {
                unreachable!("validated above")
            }
        }
    }
}

/// Procesa un payload, aplica acelerador opcional y registra latencia total.
///
/// # Inputs
///
/// - `job`: configuración del payload.
/// - `writer`: recorder de métricas del hilo.
/// - `accelerator`: hook post-procesamiento.
///
/// # Returns
///
/// Latencia total registrada, o error de métricas/acelerador.
pub fn process_record_with_accelerator(
    job: &WorkloadConfig,
    writer: &mut Recorder<u64>,
    accelerator: &dyn AcceleratorHook,
) -> Result<Duration, BenchmarkError> {
    use crate::metrics::LatencyRecorder;
    use crate::workload::process_payload;

    let parse_latency = process_payload(job);
    let accel_latency = accelerator.post_process(job, parse_latency)?;
    let total = parse_latency.saturating_add(accel_latency);

    LatencyRecorder::record_duration(writer, total).map_err(BenchmarkError::from)?;
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workload::PacketFormat;

    #[test]
    fn default_accelerator_is_none() {
        assert_eq!(AcceleratorConfig::default().mode, AcceleratorMode::None);
    }

    #[cfg(not(feature = "gpu-cuda"))]
    #[test]
    fn cuda_kernels_without_feature_is_rejected() {
        let config = AcceleratorConfig {
            mode: AcceleratorMode::CudaKernels,
            ..AcceleratorConfig::default()
        };
        let err = config.validate().unwrap_err();
        assert!(matches!(err, BenchmarkError::InvalidConfig(_)));
    }

    #[cfg(not(feature = "gpu-serious"))]
    #[test]
    fn cuda_serious_without_feature_is_rejected() {
        let config = AcceleratorConfig {
            mode: AcceleratorMode::CudaSerious,
            ..AcceleratorConfig::default()
        };
        let err = config.validate().unwrap_err();
        assert!(matches!(err, BenchmarkError::InvalidConfig(_)));
    }

    #[test]
    fn none_accelerator_adds_zero_latency() {
        let hook = NoAccelerator;
        let job = WorkloadConfig::new(256, PacketFormat::JsonLike, 0).unwrap();
        let extra = hook
            .post_process(&job, Duration::from_micros(10))
            .expect("no-op ok");
        assert_eq!(extra, Duration::ZERO);
    }
}
