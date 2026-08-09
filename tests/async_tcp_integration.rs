#![cfg(feature = "ingress-async")]

use allocator_benchmarks::ingress::{IngressConfig, IngressMode};
use allocator_benchmarks::runner::{run_benchmark, BenchmarkConfig};
use allocator_benchmarks::workload::PacketFormat;

#[test]
fn async_tcp_ingress_end_to_end() {
    let config = BenchmarkConfig {
        threads: 2,
        iterations: 256,
        payload_bytes: 512,
        format: PacketFormat::JsonLike,
        seed: 11,
        queue_depth: 32,
        ingress: IngressConfig {
            mode: IngressMode::AsyncTcp,
            bind_port: 29_876,
            connections: 4,
            read_batch_size: 16,
        },
        ..BenchmarkConfig::default()
    };

    let result = run_benchmark(&config).expect("async-tcp benchmark should succeed");
    assert_eq!(result.snapshot.count, 256);
    assert!(result.throughput_per_sec > 0.0);
}
