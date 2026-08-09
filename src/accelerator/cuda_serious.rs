//! Benchmark GPU sostenido: VRAM persistente y pipeline multi-kernel.

use crate::accelerator::cuda_common::{
    build_host_batch, map_cuda_err, SERIOUS_KERNEL_FOLD, SERIOUS_KERNEL_MIX,
    SERIOUS_KERNEL_SCRAMBLE, SERIOUS_MIN_BATCH, SERIOUS_MODULE, SERIOUS_PIPELINE_ROUNDS,
    SERIOUS_PTX,
};
use crate::accelerator::AcceleratorConfig;
use crate::accelerator::AcceleratorHook;
use crate::runner::BenchmarkError;
use crate::workload::WorkloadConfig;
use cudarc::driver::{CudaDevice, CudaFunction, CudaSlice, LaunchAsync, LaunchConfig};
use cudarc::nvrtc::Ptx;
use std::hint::black_box;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Estado GPU con buffers VRAM persistentes y funciones del pipeline.
struct SeriousGpuInner {
    device: Arc<CudaDevice>,
    batch_size: usize,
    buffers: [CudaSlice<u32>; 3],
    mix_fn: CudaFunction,
    scramble_fn: CudaFunction,
    fold_fn: CudaFunction,
}

/// Pipeline CUDA sostenido serializado por mutex para uso multi-worker seguro.
pub struct CudaSeriousAccelerator {
    inner: Mutex<SeriousGpuInner>,
}

impl CudaSeriousAccelerator {
    /// Inicializa dispositivo, buffers VRAM persistentes y kernels del pipeline.
    ///
    /// # Inputs
    ///
    /// - `config`: índice CUDA y tamaño de batch (mínimo [`SERIOUS_MIN_BATCH`]).
    ///
    /// # Returns
    ///
    /// Acelerador listo para pipeline sostenido, o error de configuración/CUDA.
    pub fn new(config: &AcceleratorConfig) -> Result<Self, BenchmarkError> {
        if config.gpu_batch_size < SERIOUS_MIN_BATCH {
            return Err(BenchmarkError::InvalidConfig(format!(
                "cuda-serious requires gpu-batch-size >= {SERIOUS_MIN_BATCH}"
            )));
        }

        let device = CudaDevice::new(config.cuda_device as usize).map_err(map_cuda_err)?;
        device
            .load_ptx(
                Ptx::from_src(SERIOUS_PTX),
                SERIOUS_MODULE,
                &[
                    SERIOUS_KERNEL_MIX,
                    SERIOUS_KERNEL_SCRAMBLE,
                    SERIOUS_KERNEL_FOLD,
                ],
            )
            .map_err(map_cuda_err)?;

        let batch_size = config.gpu_batch_size;

        Ok(Self {
            inner: Mutex::new(SeriousGpuInner {
                buffers: [
                    device.alloc_zeros::<u32>(batch_size).map_err(map_cuda_err)?,
                    device.alloc_zeros::<u32>(batch_size).map_err(map_cuda_err)?,
                    device.alloc_zeros::<u32>(batch_size).map_err(map_cuda_err)?,
                ],
                mix_fn: load_function(&device, SERIOUS_KERNEL_MIX)?,
                scramble_fn: load_function(&device, SERIOUS_KERNEL_SCRAMBLE)?,
                fold_fn: load_function(&device, SERIOUS_KERNEL_FOLD)?,
                device,
                batch_size,
            }),
        })
    }
}

impl AcceleratorHook for CudaSeriousAccelerator {
    /// H2D in-place, pipeline multi-kernel sostenido y D2H parcial del checksum.
    fn post_process(
        &self,
        job: &WorkloadConfig,
        _parse_latency: Duration,
    ) -> Result<Duration, BenchmarkError> {
        let started = Instant::now();
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| BenchmarkError::WorkerPanicked)?;

        let host_in = build_host_batch(job, inner.batch_size);
        let device = Arc::clone(&inner.device);
        device
            .htod_sync_copy_into(&host_in, &mut inner.buffers[0])
            .map_err(map_cuda_err)?;

        let cfg = LaunchConfig::for_num_elems(inner.batch_size as u32);
        let n_i32 = inner.batch_size as i32;
        let round_seed = job.seed as u32 ^ job.payload_bytes as u32;
        let mix_fn = inner.mix_fn.clone();
        let scramble_fn = inner.scramble_fn.clone();
        let fold_fn = inner.fold_fn.clone();

        for round in 0..SERIOUS_PIPELINE_ROUNDS {
            let round_tag = round_seed.wrapping_add(round);
            let (buf0, tail) = inner.buffers.split_at_mut(1);
            let (buf1, buf2) = tail.split_at_mut(1);

            unsafe {
                mix_fn
                    .clone()
                    .launch(cfg, (&buf0[0], &mut buf1[0], n_i32, round_tag))
                    .map_err(map_cuda_err)?;
                scramble_fn
                    .clone()
                    .launch(cfg, (&buf1[0], &mut buf2[0], n_i32))
                    .map_err(map_cuda_err)?;
                fold_fn
                    .clone()
                    .launch(cfg, (&buf2[0], &mut buf0[0], n_i32))
                    .map_err(map_cuda_err)?;
            }
        }

        let mut checksum = vec![0_u32; inner.batch_size];
        device
            .dtoh_sync_copy_into(&inner.buffers[0], &mut checksum)
            .map_err(map_cuda_err)?;

        black_box(
            checksum
                .iter()
                .take(8)
                .fold(0_u64, |acc, value| acc.wrapping_add(*value as u64)),
        );
        Ok(started.elapsed())
    }
}

fn load_function(
    device: &Arc<CudaDevice>,
    name: &'static str,
) -> Result<CudaFunction, BenchmarkError> {
    device
        .get_func(SERIOUS_MODULE, name)
        .ok_or_else(|| BenchmarkError::InvalidConfig(format!("cuda kernel {name} not loaded")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accelerator::cuda_common::cuda_device_available;
    use crate::accelerator::AcceleratorMode;
    use crate::workload::PacketFormat;

    #[test]
    fn rejects_batch_below_minimum() {
        let config = AcceleratorConfig {
            mode: AcceleratorMode::CudaSerious,
            gpu_batch_size: SERIOUS_MIN_BATCH - 1,
            ..AcceleratorConfig::default()
        };
        let result = CudaSeriousAccelerator::new(&config);
        assert!(matches!(result, Err(BenchmarkError::InvalidConfig(_))));
    }

    #[test]
    fn serious_accelerator_runs_when_device_available() {
        if !cuda_device_available(0) {
            eprintln!("skipping cuda-serious test: no CUDA device available");
            return;
        }

        let config = AcceleratorConfig {
            mode: AcceleratorMode::CudaSerious,
            cuda_device: 0,
            gpu_batch_size: SERIOUS_MIN_BATCH,
        };
        let accelerator = CudaSeriousAccelerator::new(&config).expect("cuda-serious init");
        let job = WorkloadConfig::new(512, PacketFormat::JsonLike, 9).expect("valid");
        let latency = accelerator
            .post_process(&job, Duration::ZERO)
            .expect("serious gpu post-process");
        assert!(latency > Duration::ZERO);
    }
}
