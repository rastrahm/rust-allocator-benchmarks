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
- Linux (for `perf stat` and io_uring ingress)
- `perf` — `linux-tools` package for your kernel
- **GPU (optional):** NVIDIA driver + CUDA-capable GPU (`cuda-kernels` / `cuda-serious` modes)
- **io_uring (optional):** Linux kernel ≥ 5.10 (`io-uring` mode)

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

# Async TCP ingress (Network C)
cargo build --release --features ingress-async

# io_uring ingress (Network D)
cargo build --release --features ingress-io-uring

# CUDA kernels + VRAM (GPU C)
cargo build --release --features gpu-cuda

# Sustained multi-kernel GPU benchmark (GPU D)
cargo build --release --features gpu-serious
```

### Feature flags

| Feature | Description | Optional dependencies |
|---------|-------------|----------------------|
| `use-system` (default) | glibc allocator | — |
| `use-jemalloc` | jemalloc allocator | `tikv-jemallocator` |
| `use-mimalloc` | mimalloc allocator | `mimalloc` |
| `ingress-async` | Async TCP ingestion (Tokio) | `tokio` |
| `ingress-io-uring` | Production-like io_uring ingestion | `tokio-uring` (+ `ingress-async`) |
| `gpu-cuda` | CUDA kernel + VRAM accelerator | `cudarc` |
| `gpu-serious` | Sustained multi-kernel GPU pipeline | `gpu-cuda` |

> Ingress and accelerator modes are selected at **runtime** (CLI). They require building with the matching feature.

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
| `--ingress` | Ingestion mode: `in-memory`, `async-tcp`, `io-uring` | in-memory |
| `--ingress-port` | TCP port (network modes) | 9876 |
| `--ingress-connections` | Concurrent client connections | 4 |
| `--ingress-read-batch` | Frames read per TCP batch | 64 |
| `--accelerator` | Post-process: `none`, `cuda-kernels`, `cuda-serious` | none |
| `--cuda-device` | CUDA device index | 0 |
| `--gpu-batch-size` | Elements per GPU batch | 256 |
| `--verbose` | Print active allocator to stderr | false |

### Network ingestion modes

| CLI mode | Feature | Description |
|----------|---------|-------------|
| `in-memory` | (default) | In-process crossbeam channel (original behavior) |
| `async-tcp` | `ingress-async` | Local TCP listener + N Tokio clients, binary wire frames |
| `io-uring` | `ingress-io-uring` | Same TCP topology via `tokio-uring` (submission-based I/O) |

### GPU accelerator modes

| CLI mode | Feature | Description |
|----------|---------|-------------|
| `none` | (default) | No GPU work after parsing |
| `cuda-kernels` | `gpu-cuda` | 1 CUDA kernel + VRAM alloc/free per payload |
| `cuda-serious` | `gpu-serious` | 3 persistent VRAM buffers, 3×3 kernel pipeline (min batch 64) |

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

# Async TCP ingress (requires --features ingress-async)
cargo run --release --features ingress-async -- \
  --ingress async-tcp --ingress-port 9876 --ingress-connections 4 \
  -n 10000 -t 24 -s 2048

# io_uring ingress (requires --features ingress-io-uring)
cargo run --release --features ingress-io-uring -- \
  --ingress io-uring --ingress-port 9877 --ingress-connections 4 \
  -n 10000 -t 24 -s 2048

# CUDA kernels + VRAM (requires --features gpu-cuda and NVIDIA GPU)
cargo run --release --features gpu-cuda -- \
  --accelerator cuda-kernels --gpu-batch-size 256 \
  -n 5000 -t 8 -s 512

# Sustained GPU benchmark (requires --features gpu-serious)
cargo run --release --features gpu-serious -- \
  --accelerator cuda-serious --gpu-batch-size 128 \
  -n 5000 -t 8 -s 512
```

Combined example (realistic network + GPU):

```bash
cargo run --release --features ingress-async,gpu-cuda -- \
  --ingress async-tcp --accelerator cuda-kernels \
  -n 10000 -t 24 -s 2048
```

### Output

The binary prints allocator, ingress mode, accelerator, throughput, latency percentiles (p50, p90, p99, p99.9), and basic stats (min, max, mean).

## Architecture

```
                    ┌─────────────────────────────────────────┐
                    │           Ingress (configurable)         │
                    │  in-memory │ async-tcp │ io-uring        │
                    └────────────────────┬────────────────────┘
                                         │ bounded channel
                                         ▼
┌─────────────┐     crossbeam-channel     ┌──────────────┐     ┌─────────────────┐
│  Producer   │ ──────────────────────► │   Workers    │ ──► │  Accelerator    │
│             │                         │  (N threads) │     │ none / cuda-*   │
└─────────────┘                         └──────┬───────┘     └─────────────────┘
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
| `src/ingress/` | Ingestion sources: in-memory, async TCP, io_uring |
| `src/accelerator/` | Post-process hooks: no-op, CUDA kernels, serious pipeline |
| `src/main.rs` | CLI and global allocator binding |
| `benches/` | Criterion micro-benchmarks |

## Tests

```bash
cargo test
cargo clippy -- -D warnings

# With network and GPU extensions
cargo test --features ingress-io-uring
cargo test --features gpu-serious
cargo clippy --features gpu-serious -- -D warnings
```

Includes unit tests (`metrics`, `workload`, `runner`, `ingress`, `accelerator`) and end-to-end integration tests:

| Test | Required feature |
|------|------------------|
| `tests/benchmark_integration.rs` | (default) |
| `tests/async_tcp_integration.rs` | `ingress-async` |
| `tests/io_uring_integration.rs` | `ingress-io-uring` |
| `tests/cuda_integration.rs` | `gpu-cuda` |
| `tests/cuda_serious_integration.rs` | `gpu-serious` |

CUDA tests are skipped automatically when no GPU is available.

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

# Ingress / accelerator profiles (cargo features resolved automatically)
./scripts/run_benchmarks.sh --profile network-async -- -n 10000 -t 24
./scripts/run_benchmarks.sh --profile gpu-serious -- -n 5000 -t 8
./scripts/run_benchmarks.sh --ingress io-uring --accelerator cuda-kernels -- -n 5000 -t 8
```

Script options (in addition to binary flags after `--`):

| Flag | Description |
|------|-------------|
| `--profile NAME` | `baseline`, `network-async`, `network-io-uring`, `gpu-kernels`, `gpu-serious`, `network-gpu` |
| `--ingress MODE` | `in-memory`, `async-tcp`, `io-uring` |
| `--accelerator MODE` | `none`, `cuda-kernels`, `cuda-serious` |
| `--no-build` | Skip `cargo build` (use an already-built binary) |
| `--runs N` | `perf stat` repetitions per allocator |

Produces a comparison table with hardware counters:

- `cycles`, `instructions`
- `cache-references`, `cache-misses`
- `page-faults`, `context-switches`, `cpu-migrations`

Results are saved to `target/perf-results/comparison_<timestamp>.txt`.

### Why does it compile on every run?

`run_benchmarks.sh` **builds by default** (`DO_BUILD=1`) and runs **three separate release builds** per invocation — one per allocator (`use-system`, `use-jemalloc`, `use-mimalloc`). The global allocator is chosen at compile time (`#[global_allocator]`), not at runtime, and all three write to the same binary path (`target/release/allocator-benchmarks`), so each measurement requires a rebuild before `perf stat` runs.

When you add ingress or GPU flags (`--profile gpu-serious`, `--ingress async-tcp`, etc.), Cargo produces **additional build configurations** (e.g. `use-jemalloc,gpu-serious`). With this project's release profile (`lto = "fat"`, `codegen-units = 1`), each combination can take several seconds on the first build.

**Expected behavior:**

| Situation | What you'll see |
|-----------|-----------------|
| First full run (3 allocators) | 3× `Compiling` + `Finished` (~10 s each with LTO) |
| Second run with the same flags | Cargo usually prints `Finished` immediately (incremental cache) |
| You change `--profile`, `--ingress`, or `--accelerator` | New feature combination → rebuild |

**Skip compilation** (single allocator only, no 3-way comparison):

```bash
cargo build --release --no-default-features --features use-jemalloc,gpu-serious
./scripts/run_benchmarks.sh --no-build --accelerator cuda-serious
```

> `--no-build` runs `perf stat` against the existing binary. It cannot compare all 3 allocators in one pass, because only the last manually built allocator remains in `target/release/`.

### Long campaign (recommended for serious analysis)

```bash
./scripts/run_benchmarks_long.sh
./scripts/run_benchmarks_long.sh --quick      # reduced version (~5 min)
./scripts/run_benchmarks_long.sh --extended   # adds network and GPU scenarios
```

Runs scenarios with `nproc` threads, 3 repetitions per allocator, and cooldown between runs.

**Baseline** (original behavior):

| Scenario | Format | Payload | Iterations | Ingress | Accelerator |
|----------|--------|---------|------------|---------|-------------|
| A | JSON | 2 KiB | 10 M | in-memory | none |
| B | Borsh | 2 KiB | 10 M | in-memory | none |
| C | JSON | 8 KiB | 2 M | in-memory | none |

**With `--extended`** (GPU scenarios skipped when no NVIDIA GPU is present):

| Scenario | Ingress | Accelerator |
|----------|---------|-------------|
| D | async-tcp | none |
| E | io-uring | none |
| F | in-memory | cuda-kernels |
| G | in-memory | cuda-serious |
| H | async-tcp | cuda-kernels |

Output is written to `target/perf-results/long_<timestamp>/`.

## Scripts

| Script | Description |
|--------|-------------|
| `scripts/run_benchmarks.sh` | Compare all 3 allocators under `perf stat`; supports ingress/GPU profiles |
| `scripts/run_benchmarks_long.sh` | Multi-scenario campaign with statistical repetition; `--extended` for network/GPU |
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
| `tokio` | Async TCP ingestion (optional) |
| `tokio-uring` | io_uring ingestion (optional) |
| `cudarc` | CUDA kernels and VRAM allocation (optional) |

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
