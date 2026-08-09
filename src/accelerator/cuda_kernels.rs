//! Kernels CUDA + alloc/liberación VRAM (GPU C).

use crate::accelerator::cuda_common::{
    build_host_batch, map_cuda_err, BASIC_KERNEL, BASIC_MODULE, BASIC_PTX,
};
use crate::accelerator::AcceleratorConfig;
use crate::accelerator::AcceleratorHook;
use crate::runner::BenchmarkError;
use crate::workload::WorkloadConfig;
use cudarc::driver::{CudaDevice, LaunchAsync, LaunchConfig};
use cudarc::nvrtc::Ptx;
use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Acelerador CUDA: copia batch a VRAM, ejecuta kernel de mezcla y recupera resultado.
pub struct CudaKernelAccelerator {
    device: Arc<CudaDevice>,
    batch_size: usize,
}

impl CudaKernelAccelerator {
    /// Inicializa dispositivo CUDA y carga el módulo PTX embebido.
    ///
    /// # Inputs
    ///
    /// - `config`: índice de dispositivo y tamaño de batch GPU.
    ///
    /// # Returns
    ///
    /// Acelerador listo, o error si CUDA no está disponible.
    pub fn new(config: &AcceleratorConfig) -> Result<Self, BenchmarkError> {
        let device = CudaDevice::new(config.cuda_device as usize).map_err(map_cuda_err)?;
        device
            .load_ptx(Ptx::from_src(BASIC_PTX), BASIC_MODULE, &[BASIC_KERNEL])
            .map_err(map_cuda_err)?;

        Ok(Self {
            device,
            batch_size: config.gpu_batch_size,
        })
    }
}

impl AcceleratorHook for CudaKernelAccelerator {
    /// Copia un batch derivado del payload a VRAM, lanza kernel y mide la latencia GPU.
    fn post_process(
        &self,
        job: &WorkloadConfig,
        _parse_latency: Duration,
    ) -> Result<Duration, BenchmarkError> {
        let started = Instant::now();
        let n = self.batch_size;
        let host_in = build_host_batch(job, n);

        let input_dev = self.device.htod_copy(host_in).map_err(map_cuda_err)?;
        let mut output_dev = self
            .device
            .alloc_zeros::<u32>(n)
            .map_err(map_cuda_err)?;

        let function = self
            .device
            .get_func(BASIC_MODULE, BASIC_KERNEL)
            .ok_or_else(|| {
                BenchmarkError::InvalidConfig("cuda kernel payload_mix not loaded".into())
            })?;

        let cfg = LaunchConfig::for_num_elems(n as u32);
        unsafe {
            function
                .launch(cfg, (&input_dev, &mut output_dev, n as i32))
                .map_err(map_cuda_err)?;
        }

        let host_out = self.device.sync_reclaim(output_dev).map_err(map_cuda_err)?;
        self.device.sync_reclaim(input_dev).map_err(map_cuda_err)?;

        black_box(
            host_out
                .iter()
                .fold(0_u64, |acc, value| acc.wrapping_add(*value as u64)),
        );
        Ok(started.elapsed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accelerator::cuda_common::cuda_device_available;
    use crate::workload::PacketFormat;

    #[test]
    fn build_host_batch_is_deterministic() {
        let job = WorkloadConfig::new(512, PacketFormat::JsonLike, 42).expect("valid");
        let a = build_host_batch(&job, 8);
        let b = build_host_batch(&job, 8);
        assert_eq!(a, b);
        assert_eq!(a.len(), 8);
    }

    #[test]
    fn cuda_kernel_accelerator_runs_when_device_available() {
        if !cuda_device_available(0) {
            eprintln!("skipping cuda test: no CUDA device available");
            return;
        }

        let config = AcceleratorConfig {
            mode: crate::accelerator::AcceleratorMode::CudaKernels,
            cuda_device: 0,
            gpu_batch_size: 64,
        };
        let accelerator = CudaKernelAccelerator::new(&config).expect("cuda init");
        let job = WorkloadConfig::new(256, PacketFormat::JsonLike, 7).expect("valid");
        let latency = accelerator
            .post_process(&job, Duration::ZERO)
            .expect("gpu post-process");
        assert!(latency > Duration::ZERO);
    }
}
