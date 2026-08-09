#![cfg(feature = "ingress-io-uring")]

use allocator_benchmarks::ingress::{IngressConfig, IngressMode};
use allocator_benchmarks::runner::{run_benchmark, BenchmarkConfig};
use allocator_benchmarks::workload::PacketFormat;

#[test]
fn io_uring_ingress_end_to_end() {
    let config = BenchmarkConfig {
        threads: 2,
        iterations: 256,
        payload_bytes: 512,
        format: PacketFormat::JsonLike,
        seed: 17,
        queue_depth: 32,
        ingress: IngressConfig {
            mode: IngressMode::IoUring,
            bind_port: 49_876,
            connections: 4,
            read_batch_size: 16,
        },
        ..BenchmarkConfig::default()
    };

    let result = run_benchmark(&config).expect("io-uring benchmark should succeed");
    assert_eq!(result.snapshot.count, 256);
    assert!(result.throughput_per_sec > 0.0);
}
