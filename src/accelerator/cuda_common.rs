//! Utilidades compartidas entre modos CUDA C y D.

use crate::runner::BenchmarkError;
use crate::workload::WorkloadConfig;
use cudarc::driver::CudaDevice;

pub(crate) const BASIC_MODULE: &str = "payload_kernels";
pub(crate) const BASIC_KERNEL: &str = "payload_mix";
pub(crate) const BASIC_PTX: &str = include_str!("payload_mix.ptx");

pub(crate) const SERIOUS_MODULE: &str = "payload_serious";
pub(crate) const SERIOUS_KERNEL_MIX: &str = "serious_mix";
pub(crate) const SERIOUS_KERNEL_SCRAMBLE: &str = "serious_scramble";
pub(crate) const SERIOUS_KERNEL_FOLD: &str = "serious_fold";
pub(crate) const SERIOUS_PTX: &str = include_str!("payload_serious.ptx");

/// Mínimo de elementos por batch en modo cuda-serious.
pub(crate) const SERIOUS_MIN_BATCH: usize = 64;

/// Número de rondas del pipeline multi-kernel en modo serio.
pub(crate) const SERIOUS_PIPELINE_ROUNDS: u32 = 3;

/// Construye el batch host determinista a partir de la configuración del payload.
pub(crate) fn build_host_batch(job: &WorkloadConfig, batch_size: usize) -> Vec<u32> {
    (0..batch_size)
        .map(|index| {
            let word = job
                .seed
                .wrapping_add(index as u64)
                .wrapping_mul(job.payload_bytes as u64 + 1);
            word as u32 ^ (job.payload_bytes as u32).rotate_left((index % 32) as u32)
        })
        .collect()
}

pub(crate) fn map_cuda_err(err: cudarc::driver::DriverError) -> BenchmarkError {
    BenchmarkError::InvalidConfig(format!("cuda driver error: {err}"))
}

/// Indica si hay un dispositivo CUDA accesible (para tests condicionales).
pub fn cuda_device_available(device_index: u32) -> bool {
    CudaDevice::new(device_index as usize).is_ok()
}
