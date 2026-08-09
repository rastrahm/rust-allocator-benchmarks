#![cfg(feature = "gpu-cuda")]

use allocator_benchmarks::accelerator::{cuda_device_available, AcceleratorConfig, AcceleratorMode};
use allocator_benchmarks::runner::{run_benchmark, BenchmarkConfig};
use allocator_benchmarks::workload::PacketFormat;

#[test]
fn cuda_kernels_accelerator_end_to_end() {
    if !cuda_device_available(0) {
        eprintln!("skipping cuda integration test: no CUDA device available");
        return;
    }

    let config = BenchmarkConfig {
        threads: 2,
        iterations: 128,
        payload_bytes: 512,
        format: PacketFormat::JsonLike,
        seed: 3,
        queue_depth: 32,
        accelerator: AcceleratorConfig {
            mode: AcceleratorMode::CudaKernels,
            cuda_device: 0,
            gpu_batch_size: 128,
        },
        ..BenchmarkConfig::default()
    };

    let result = run_benchmark(&config).expect("cuda-kernels benchmark should succeed");
    assert_eq!(result.snapshot.count, 128);
    assert!(result.throughput_per_sec > 0.0);
    assert!(result.snapshot.p50_ns > 0);
}
