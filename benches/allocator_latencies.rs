//! Micro-benchmarks Criterion del coste de alloc/dealloc por payload y formato.
//!
//! Ejecutar con distintos allocators:
//!   cargo bench --no-default-features --features use-system
//!   cargo bench --no-default-features --features use-jemalloc
//!   cargo bench --no-default-features --features use-mimalloc

use allocator_benchmarks::workload::{process_payload, PacketFormat, WorkloadConfig};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::time::Duration;

#[cfg(feature = "use-jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(feature = "use-mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

const PAYLOAD_SIZES: [usize; 4] = [256, 512, 2_048, 8_192];
const PARALLEL_WORKERS: usize = 4;

/// Configura parámetros comunes de Criterion alineados con mediciones de latencia.
fn configure_latency_group(group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>) {
    group
        .warm_up_time(Duration::from_millis(300))
        .measurement_time(Duration::from_secs(3))
        .sample_size(100);
}

/// Benchmarks de parsing simulado estilo JSON a distintos tamaños de payload.
fn bench_json_payload_sizes(c: &mut Criterion) {
    let mut group = c.benchmark_group("json_payload");
    configure_latency_group(&mut group);

    for size in PAYLOAD_SIZES {
        let config =
            WorkloadConfig::new(size, PacketFormat::JsonLike, 42).expect("valid payload size");
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::new("process", size), &config, |bencher, cfg| {
            bencher.iter(|| black_box(process_payload(cfg)));
        });
    }

    group.finish();
}

/// Benchmarks de parsing simulado estilo Borsh a distintos tamaños de payload.
fn bench_borsh_payload_sizes(c: &mut Criterion) {
    let mut group = c.benchmark_group("borsh_payload");
    configure_latency_group(&mut group);

    for size in PAYLOAD_SIZES {
        let config =
            WorkloadConfig::new(size, PacketFormat::BorshLike, 42).expect("valid payload size");
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::new("process", size), &config, |bencher, cfg| {
            bencher.iter(|| black_box(process_payload(cfg)));
        });
    }

    group.finish();
}

/// Benchmark de throughput paralelo con rayon (4 workers, payload 2 KiB JSON).
fn bench_parallel_json_ingestion(c: &mut Criterion) {
    let mut group = c.benchmark_group("parallel_json");
    group
        .warm_up_time(Duration::from_millis(300))
        .measurement_time(Duration::from_secs(3))
        .sample_size(50)
        .throughput(Throughput::Elements(PARALLEL_WORKERS as u64));

    let base =
        WorkloadConfig::new(2_048, PacketFormat::JsonLike, 0).expect("valid payload size");

    group.bench_function("rayon_4_workers", |bencher| {
        bencher.iter(|| {
            let base = base.clone();
            rayon::scope(|scope| {
                for worker in 0..PARALLEL_WORKERS {
                    let cfg_base = base.clone();
                    scope.spawn(move |_| {
                        let config = WorkloadConfig {
                            seed: worker as u64,
                            ..cfg_base
                        };
                        black_box(process_payload(&config));
                    });
                }
            });
        });
    });

    group.finish();
}

/// Benchmark de throughput paralelo con rayon (4 workers, payload 2 KiB Borsh).
fn bench_parallel_borsh_ingestion(c: &mut Criterion) {
    let mut group = c.benchmark_group("parallel_borsh");
    group
        .warm_up_time(Duration::from_millis(300))
        .measurement_time(Duration::from_secs(3))
        .sample_size(50)
        .throughput(Throughput::Elements(PARALLEL_WORKERS as u64));

    let base =
        WorkloadConfig::new(2_048, PacketFormat::BorshLike, 0).expect("valid payload size");

    group.bench_function("rayon_4_workers", |bencher| {
        bencher.iter(|| {
            let base = base.clone();
            rayon::scope(|scope| {
                for worker in 0..PARALLEL_WORKERS {
                    let cfg_base = base.clone();
                    scope.spawn(move |_| {
                        let config = WorkloadConfig {
                            seed: worker as u64,
                            ..cfg_base
                        };
                        black_box(process_payload(&config));
                    });
                }
            });
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_json_payload_sizes,
    bench_borsh_payload_sizes,
    bench_parallel_json_ingestion,
    bench_parallel_borsh_ingestion,
);
criterion_main!(benches);
