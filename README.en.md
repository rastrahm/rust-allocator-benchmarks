# allocator-benchmarks

> English version. See [README.md](README.md) for Spanish.

A Rust benchmark suite for comparing global memory allocators under concurrent network-ingestion workloads. It simulates the pipeline of a high-performance blockchain node or low-latency trading system: thousands of short-lived payloads, intensive alloc/dealloc patterns, and tail-latency measurement (p99 / p99.9).

## Goals

- Generate concurrent load with realistic alloc/dealloc patterns (JSON-like and Borsh-like).
- Compare three global allocators via compile-time feature flags.
- Measure latency with high-resolution histograms (`hdrhistogram`).
- Run statistical micro-benchmarks with Criterion.
- Capture Linux kernel hardware counters with `perf stat`.

## Allocators compared

| Feature | Allocator | Notes |
|---------|-----------|-------|
| `use-system` (default) | glibc malloc | System baseline |
| `use-jemalloc` | [tikv-jemallocator](https://github.com/tikv/jemallocator) | Strong for long-running multithreaded workloads |
| `use-mimalloc` | [mimalloc](https://github.com/microsoft/mimalloc) | Low latency, good multi-core behavior |

> The allocator is selected at **compile time**, not runtime. Build a separate binary for each allocator.

## Requirements

- Rust 2021 (stable)
- Linux (for `perf stat`)
- `perf` — `linux-tools` package for your kernel

### Installing perf (Ubuntu / Pop!_OS)

```bash
sudo apt update
sudo apt install linux-tools-common linux-tools-virtual-6.17
sudo sysctl kernel.perf_event_paranoid=1
```

On Pop!_OS with a custom kernel (e.g. 6.18.x), the `/usr/bin/perf` wrapper may fail to find the binary. Fix:

```bash
sudo ./scripts/fix-perf-wrapper.sh
```

## Build

```bash
# System allocator (default)
cargo build --release

# jemalloc
cargo build --release --no-default-features --features use-jemalloc

# mimalloc
cargo build --release --no-default-features --features use-mimalloc
```

The `release` profile is tuned for benchmarking and profiling:

- `opt-level = 3`, `lto = "fat"`, `codegen-units = 1`
- `debug = true` — symbols for `perf` and flamegraphs

## CLI usage

```bash
cargo run --release -- [OPTIONS]
```

| Flag | Description | Default |
|------|-------------|---------|
| `-t, --threads` | Consumer worker threads | Available CPUs |
| `-n, --iterations` | Total payloads to process | 10,000 |
| `-s, --payload-size` | Simulated payload size (bytes) | 2,048 |
| `-f, --format` | `json-like` or `borsh-like` | json-like |
| `--seed` | Base seed for payload content | 0 |
| `--queue-depth` | Bounded channel capacity | threads × 4 |
| `--verbose` | Print active allocator to stderr | false |

### Examples

```bash
# Quick run
cargo run --release -- -n 5000 -t 4 -s 512 --verbose

# jemalloc, Borsh format
cargo run --release --no-default-features --features use-jemalloc -- \
  -n 50000 -t 8 -s 4096 -f borsh-like

# mimalloc
cargo run --release --no-default-features --features use-mimalloc -- \
  -n 20000 -t 24 -s 2048
```

### Output

The binary prints throughput, latency percentiles (p50, p90, p99, p99.9), and basic stats (min, max, mean).

## Architecture

```
┌─────────────┐     bounded channel     ┌──────────────┐
│  Producer   │ ──────────────────────► │   Workers    │
│  (1 thread) │   crossbeam-channel     │  (N threads) │
└─────────────┘                         └──────┬───────┘
                                               │
                                               ▼
                                      ┌────────────────┐
                                      │ LatencyRecorder│
                                      │  (HdrHistogram)│
                                      └────────────────┘
```

| Module | Responsibility |
|--------|----------------|
| `src/workload.rs` | Simulates JSON/Borsh parsing with intensive alloc/dealloc |
| `src/metrics.rs` | Thread-safe latency recording and percentile calculation |
| `src/runner.rs` | Producer/consumer pipeline and benchmark orchestration |
| `src/main.rs` | CLI and global allocator binding |
| `benches/` | Criterion micro-benchmarks |

## Tests

```bash
cargo test
cargo clippy -- -D warnings
```

Includes unit tests (`metrics`, `workload`, `runner`) and end-to-end integration tests in `tests/benchmark_integration.rs`.

## Micro-benchmarks (Criterion)

```bash
cargo bench --bench allocator_latencies

# Per allocator
cargo bench --no-default-features --features use-jemalloc -- json_payload
cargo bench --no-default-features --features use-mimalloc  -- borsh_payload
```

Benchmark groups:

- `json_payload/process/{256,512,2048,8192}` — latency by size, JSON format
- `borsh_payload/process/{256,512,2048,8192}` — Borsh format
- `parallel_json/rayon_4_workers` — parallel ingestion with rayon
- `parallel_borsh/rayon_4_workers`

HTML reports are written to `target/criterion/`.

## Benchmarks with perf stat

### Standard run (3 allocators)

```bash
./scripts/run_benchmarks.sh
./scripts/run_benchmarks.sh -- -n 50000 -t 8 -s 2048
./scripts/run_benchmarks.sh --runs 3 -- -n 20000 -t 24 -s 2048
```

Produces a comparison table with hardware counters:

- `cycles`, `instructions`
- `cache-references`, `cache-misses`
- `page-faults`, `context-switches`, `cpu-migrations`

Results are saved to `target/perf-results/comparison_<timestamp>.txt`.

### Long campaign (recommended for serious analysis)

```bash
./scripts/run_benchmarks_long.sh
./scripts/run_benchmarks_long.sh --quick   # reduced version (~5 min)
```

Runs three scenarios with 24 threads, 3 repetitions per allocator, and cooldown between runs:

| Scenario | Format | Payload | Iterations |
|----------|--------|---------|------------|
| A | JSON | 2 KiB | 10 M |
| B | Borsh | 2 KiB | 10 M |
| C | JSON | 8 KiB | 2 M |

Output is written to `target/perf-results/long_<timestamp>/`.

## Scripts

| Script | Description |
|--------|-------------|
| `scripts/run_benchmarks.sh` | Compare all 3 allocators under `perf stat` |
| `scripts/run_benchmarks_long.sh` | Multi-scenario campaign with statistical repetition |
| `scripts/fix-perf-wrapper.sh` | Fix the `perf` symlink on Pop!_OS |

## Key dependencies

| Crate | Purpose |
|-------|---------|
| `hdrhistogram` | Latency percentiles (p50–p99.9) with tail accuracy |
| `crossbeam-channel` | Bounded producer/consumer channel with backpressure |
| `rayon` | Parallelism in Criterion benchmarks |
| `clap` | CLI |
| `criterion` | Statistical micro-benchmarks (dev) |
| `tikv-jemallocator` / `mimalloc` | Optional alternative global allocators |

## Quick metric reference

| Metric | Source | Meaning |
|--------|--------|---------|
| p50 / p99 / p99.9 | HdrHistogram (app) | Latency experienced by the workload |
| Throughput | Wall-clock (app) | Payloads processed per second |
| cycles / instructions | perf (hardware) | Total computational cost |
| cache-misses | perf (hardware) | Pressure on the cache hierarchy |
| page-faults | perf (kernel) | Heap growth / memory mapping |
| context-switches | perf (kernel) | Scheduler contention (rises under heavy load) |

## License

MIT
