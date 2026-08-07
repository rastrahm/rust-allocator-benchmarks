use allocator_benchmarks::runner::{run_benchmark, BenchmarkConfig};
use allocator_benchmarks::workload::PacketFormat;

#[test]
fn end_to_end_benchmark_matches_iteration_count() {
    let config = BenchmarkConfig {
        threads: 2,
        iterations: 128,
        payload_bytes: 256,
        format: PacketFormat::BorshLike,
        seed: 99,
        queue_depth: 16,
    };

    let result = run_benchmark(&config).expect("integration benchmark should succeed");
    assert_eq!(result.snapshot.count, 128);
    assert!(result.elapsed > std::time::Duration::ZERO);
}

#[test]
fn concurrent_json_workload_produces_ordered_percentiles() {
    let config = BenchmarkConfig {
        threads: 4,
        iterations: 400,
        payload_bytes: 512,
        format: PacketFormat::JsonLike,
        seed: 1,
        queue_depth: 32,
    };

    let result = run_benchmark(&config).expect("integration benchmark should succeed");
    let snap = &result.snapshot;

    assert_eq!(snap.count, 400);
    assert!(snap.min_ns <= snap.p50_ns);
    assert!(snap.p50_ns <= snap.p90_ns);
    assert!(snap.p90_ns <= snap.p99_ns);
    assert!(snap.p99_ns <= snap.p999_ns);
    assert!(snap.p999_ns <= snap.max_ns);
}
