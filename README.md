# allocator-benchmarks

> Versión en inglés: [README.en.md](README.en.md)

Suite de benchmarks en Rust para comparar allocators globales de memoria bajo cargas concurrentes de ingesta de red. Simula el pipeline de un nodo blockchain o sistema de trading de baja latencia: miles de payloads de corta duración, alloc/dealloc intensivos y medición de colas largas (p99 / p99.9).

## Objetivos

- Generar carga concurrente con patrones realistas de alloc/dealloc (JSON-like y Borsh-like).
- Comparar tres allocators globales vía feature flags de compilación.
- Medir latencia con histogramas de alta resolución (`hdrhistogram`).
- Micro-benchmarks estadísticos con Criterion.
- Capturar contadores de hardware del kernel Linux con `perf stat`.

## Allocators comparados

| Feature | Allocator | Notas |
|---------|-----------|-------|
| `use-system` (default) | glibc malloc | Baseline del sistema |
| `use-jemalloc` | [tikv-jemallocator](https://github.com/tikv/jemallocator) | Orientado a cargas multihilo de larga duración |
| `use-mimalloc` | [mimalloc](https://github.com/microsoft/mimalloc) | Baja latencia, buen comportamiento multi-core |

> El allocator se elige en **tiempo de compilación**, no en runtime. Compila un binario distinto por allocator.

## Requisitos

- Rust 2021 (stable)
- Linux (para `perf stat` e ingesta io_uring)
- `perf` — paquete `linux-tools` de tu kernel
- **GPU (opcional):** driver NVIDIA + GPU CUDA-capable (modos `cuda-kernels` / `cuda-serious`)
- **io_uring (opcional):** kernel Linux ≥ 5.10 (modo `io-uring`)

### Instalar perf (Ubuntu / Pop!_OS)

```bash
sudo apt update
sudo apt install linux-tools-common linux-tools-virtual-6.17
sudo sysctl kernel.perf_event_paranoid=1
```

En Pop!_OS con kernel custom (p. ej. 6.18.x), el wrapper `/usr/bin/perf` puede no encontrar el binario. Solución:

```bash
sudo ./scripts/fix-perf-wrapper.sh
```

## Compilación

```bash
# Allocator del sistema (default)
cargo build --release

# jemalloc
cargo build --release --no-default-features --features use-jemalloc

# mimalloc
cargo build --release --no-default-features --features use-mimalloc

# Ingesta async TCP (Red C)
cargo build --release --features ingress-async

# Ingesta io_uring (Red D)
cargo build --release --features ingress-io-uring

# GPU kernels + VRAM (GPU C)
cargo build --release --features gpu-cuda

# Benchmark GPU serio multi-kernel (GPU D)
cargo build --release --features gpu-serious
```

### Feature flags

| Feature | Descripción | Dependencias opcionales |
|---------|-------------|-------------------------|
| `use-system` (default) | Allocator glibc | — |
| `use-jemalloc` | Allocator jemalloc | `tikv-jemallocator` |
| `use-mimalloc` | Allocator mimalloc | `mimalloc` |
| `ingress-async` | Ingesta TCP async (Tokio) | `tokio` |
| `ingress-io-uring` | Ingesta production-like io_uring | `tokio-uring` (+ `ingress-async`) |
| `gpu-cuda` | Acelerador CUDA kernels + VRAM | `cudarc` |
| `gpu-serious` | Pipeline GPU sostenido multi-kernel | `gpu-cuda` |

> Los modos de ingesta y acelerador se eligen en **runtime** (CLI). Requieren compilar con el feature correspondiente.

El perfil `release` está optimizado para benchmarks y profiling:

- `opt-level = 3`, `lto = "fat"`, `codegen-units = 1`
- `debug = true` — símbolos para `perf` y flamegraphs

## Uso — CLI

```bash
cargo run --release -- [OPCIONES]
```

| Flag | Descripción | Default |
|------|-------------|---------|
| `-t, --threads` | Hilos worker consumidores | CPUs disponibles |
| `-n, --iterations` | Payloads totales a procesar | 10 000 |
| `-s, --payload-size` | Tamaño del payload simulado (bytes) | 2 048 |
| `-f, --format` | `json-like` o `borsh-like` | json-like |
| `--seed` | Semilla base del contenido | 0 |
| `--queue-depth` | Capacidad del canal acotado | threads × 4 |
| `--ingress` | Modo de ingesta: `in-memory`, `async-tcp`, `io-uring` | in-memory |
| `--ingress-port` | Puerto TCP (modos de red) | 9876 |
| `--ingress-connections` | Conexiones cliente concurrentes | 4 |
| `--ingress-read-batch` | Frames leídos por batch TCP | 64 |
| `--accelerator` | Post-proceso: `none`, `cuda-kernels`, `cuda-serious` | none |
| `--cuda-device` | Índice del dispositivo CUDA | 0 |
| `--gpu-batch-size` | Elementos por batch GPU | 256 |
| `--verbose` | Muestra el allocator activo en stderr | false |

### Modos de ingesta de red

| Modo CLI | Feature | Descripción |
|----------|---------|-------------|
| `in-memory` | (default) | Canal crossbeam in-process (comportamiento original) |
| `async-tcp` | `ingress-async` | Listener TCP local + N clientes Tokio, frames wire binarios |
| `io-uring` | `ingress-io-uring` | Misma topología TCP vía `tokio-uring` (I/O submission-based) |

### Modos de aceleración GPU

| Modo CLI | Feature | Descripción |
|----------|---------|-------------|
| `none` | (default) | Sin trabajo GPU post-parseo |
| `cuda-kernels` | `gpu-cuda` | 1 kernel CUDA + alloc/liberación VRAM por payload |
| `cuda-serious` | `gpu-serious` | 3 buffers VRAM persistentes, pipeline 3×3 kernels (mín. batch 64) |

### Ejemplos

```bash
# Corrida rápida
cargo run --release -- -n 5000 -t 4 -s 512 --verbose

# jemalloc, formato Borsh
cargo run --release --no-default-features --features use-jemalloc -- \
  -n 50000 -t 8 -s 4096 -f borsh-like

# mimalloc
cargo run --release --no-default-features --features use-mimalloc -- \
  -n 20000 -t 24 -s 2048

# Ingesta async TCP (requiere --features ingress-async)
cargo run --release --features ingress-async -- \
  --ingress async-tcp --ingress-port 9876 --ingress-connections 4 \
  -n 10000 -t 24 -s 2048

# Ingesta io_uring (requiere --features ingress-io-uring)
cargo run --release --features ingress-io-uring -- \
  --ingress io-uring --ingress-port 9877 --ingress-connections 4 \
  -n 10000 -t 24 -s 2048

# GPU kernels + VRAM (requiere --features gpu-cuda y GPU NVIDIA)
cargo run --release --features gpu-cuda -- \
  --accelerator cuda-kernels --gpu-batch-size 256 \
  -n 5000 -t 8 -s 512

# Benchmark GPU serio (requiere --features gpu-serious)
cargo run --release --features gpu-serious -- \
  --accelerator cuda-serious --gpu-batch-size 128 \
  -n 5000 -t 8 -s 512
```

Combinaciones posibles (ejemplo: red realista + GPU):

```bash
cargo run --release --features ingress-async,gpu-cuda -- \
  --ingress async-tcp --accelerator cuda-kernels \
  -n 10000 -t 24 -s 2048
```

### Salida

El binario imprime allocator, modo de ingesta, acelerador, throughput, percentiles de latencia (p50, p90, p99, p99.9) y estadísticas básicas (min, max, mean).

## Arquitectura

```
                    ┌─────────────────────────────────────────┐
                    │           Ingress (configurable)         │
                    │  in-memory │ async-tcp │ io-uring        │
                    └────────────────────┬────────────────────┘
                                         │ bounded channel
                                         ▼
┌─────────────┐     crossbeam-channel     ┌──────────────┐     ┌─────────────────┐
│  Productor  │ ──────────────────────► │   Workers    │ ──► │  Accelerator    │
│             │                         │  (N hilos)   │     │ none / cuda-*   │
└─────────────┘                         └──────┬───────┘     └─────────────────┘
                                               │
                                               ▼
                                      ┌────────────────┐
                                      │ LatencyRecorder│
                                      │  (HdrHistogram)│
                                      └────────────────┘
```

| Módulo | Responsabilidad |
|--------|-----------------|
| `src/workload.rs` | Simula parsing JSON/Borsh con alloc/dealloc intensivos |
| `src/metrics.rs` | Registro thread-safe de latencias y cálculo de percentiles |
| `src/runner.rs` | Pipeline productor/consumidor y orquestación del benchmark |
| `src/ingress/` | Fuentes de ingesta: in-memory, async TCP, io_uring |
| `src/accelerator/` | Hooks post-proceso: no-op, CUDA kernels, pipeline serio |
| `src/main.rs` | CLI y binding del allocator global |
| `benches/` | Micro-benchmarks Criterion |

## Tests

```bash
cargo test
cargo clippy -- -D warnings

# Con extensiones de red y GPU
cargo test --features ingress-io-uring
cargo test --features gpu-serious
cargo clippy --features gpu-serious -- -D warnings
```

Incluye tests unitarios (`metrics`, `workload`, `runner`, `ingress`, `accelerator`) e integración end-to-end:

| Test | Feature requerido |
|------|-------------------|
| `tests/benchmark_integration.rs` | (default) |
| `tests/async_tcp_integration.rs` | `ingress-async` |
| `tests/io_uring_integration.rs` | `ingress-io-uring` |
| `tests/cuda_integration.rs` | `gpu-cuda` |
| `tests/cuda_serious_integration.rs` | `gpu-serious` |

Los tests CUDA se omiten automáticamente si no hay GPU disponible.

## Micro-benchmarks (Criterion)

```bash
cargo bench --bench allocator_latencies

# Por allocator
cargo bench --no-default-features --features use-jemalloc -- json_payload
cargo bench --no-default-features --features use-mimalloc  -- borsh_payload
```

Grupos de benchmark:

- `json_payload/process/{256,512,2048,8192}` — latencia por tamaño, formato JSON
- `borsh_payload/process/{256,512,2048,8192}` — formato Borsh
- `parallel_json/rayon_4_workers` — ingesta paralela con rayon
- `parallel_borsh/rayon_4_workers`

Reportes HTML en `target/criterion/`.

## Benchmarks con perf stat

### Corrida estándar (3 allocators)

```bash
./scripts/run_benchmarks.sh
./scripts/run_benchmarks.sh -- -n 50000 -t 8 -s 2048
./scripts/run_benchmarks.sh --runs 3 -- -n 20000 -t 24 -s 2048

# Perfiles de ingesta / acelerador (resuelven cargo features automáticamente)
./scripts/run_benchmarks.sh --profile network-async -- -n 10000 -t 24
./scripts/run_benchmarks.sh --profile gpu-serious -- -n 5000 -t 8
./scripts/run_benchmarks.sh --ingress io-uring --accelerator cuda-kernels -- -n 5000 -t 8
```

Opciones del script (además de los flags del binario, tras `--`):

| Flag | Descripción |
|------|-------------|
| `--profile NAME` | `baseline`, `network-async`, `network-io-uring`, `gpu-kernels`, `gpu-serious`, `network-gpu` |
| `--ingress MODE` | `in-memory`, `async-tcp`, `io-uring` |
| `--accelerator MODE` | `none`, `cuda-kernels`, `cuda-serious` |
| `--no-build` | Omite `cargo build` (usa el binario ya compilado) |
| `--runs N` | Repeticiones de `perf stat` por allocator |

Genera una tabla comparativa con contadores de hardware:

- `cycles`, `instructions`
- `cache-references`, `cache-misses`
- `page-faults`, `context-switches`, `cpu-migrations`

Resultados en `target/perf-results/comparison_<timestamp>.txt`.

### ¿Por qué compila en cada ejecución?

`run_benchmarks.sh` **compila por defecto** (`DO_BUILD=1`) y ejecuta **tres builds release distintos** por corrida — uno por allocator (`use-system`, `use-jemalloc`, `use-mimalloc`). El allocator global se elige en tiempo de compilación (`#[global_allocator]`), no en runtime, y los tres escriben al mismo binario (`target/release/allocator-benchmarks`), así que cada medición requiere recompilar antes de ejecutar `perf stat`.

Si añades flags de ingesta o GPU (`--profile gpu-serious`, `--ingress async-tcp`, etc.), Cargo genera **configuraciones de build adicionales** (p. ej. `use-jemalloc,gpu-serious`). Con el perfil release del proyecto (`lto = "fat"`, `codegen-units = 1`) cada combinación puede tardar varios segundos la primera vez.

**Comportamiento esperado:**

| Situación | Qué verás |
|-----------|-----------|
| Primera corrida completa (3 allocators) | 3× `Compiling` + `Finished` (~10 s cada uno con LTO) |
| Segunda corrida con los mismos flags | Cargo suele responder `Finished` al instante (caché incremental) |
| Cambias `--profile`, `--ingress` o `--accelerator` | Nueva combinación de features → recompilación |

**Evitar compilar** (solo un allocator, sin comparativa de los tres):

```bash
cargo build --release --no-default-features --features use-jemalloc,gpu-serious
./scripts/run_benchmarks.sh --no-build --accelerator cuda-serious
```

> `--no-build` ejecuta `perf stat` sobre el binario existente. No sirve para comparar los 3 allocators en una sola pasada, porque solo queda el último que compilaste manualmente.

### Campaña larga (recomendada para análisis serio)

```bash
./scripts/run_benchmarks_long.sh
./scripts/run_benchmarks_long.sh --quick      # versión reducida (~5 min)
./scripts/run_benchmarks_long.sh --extended   # añade escenarios de red y GPU
```

Ejecuta escenarios con `nproc` threads, 3 repeticiones por allocator y cooldown entre corridas.

**Baseline** (comportamiento original):

| Escenario | Formato | Payload | Iteraciones | Ingress | Accelerator |
|-----------|---------|---------|-------------|---------|-------------|
| A | JSON | 2 KiB | 10 M | in-memory | none |
| B | Borsh | 2 KiB | 10 M | in-memory | none |
| C | JSON | 8 KiB | 2 M | in-memory | none |

**Con `--extended`** (omite escenarios GPU si no hay NVIDIA):

| Escenario | Ingress | Accelerator |
|-----------|---------|-------------|
| D | async-tcp | none |
| E | io-uring | none |
| F | in-memory | cuda-kernels |
| G | in-memory | cuda-serious |
| H | async-tcp | cuda-kernels |

Salida en `target/perf-results/long_<timestamp>/`.

## Scripts

| Script | Descripción |
|--------|-------------|
| `scripts/run_benchmarks.sh` | Compara los 3 allocators bajo `perf stat`; soporta perfiles de ingesta/GPU |
| `scripts/run_benchmarks_long.sh` | Campaña multi-escenario con repetición estadística; `--extended` para red/GPU |
| `scripts/fix-perf-wrapper.sh` | Corrige el symlink de `perf` en Pop!_OS |

## Dependencias principales

| Crate | Uso |
|-------|-----|
| `hdrhistogram` | Percentiles de latencia (p50–p99.9) con precisión en colas largas |
| `crossbeam-channel` | Canal acotado productor/consumidor con backpressure |
| `rayon` | Paralelismo en benchmarks Criterion |
| `clap` | CLI |
| `criterion` | Micro-benchmarks estadísticos (dev) |
| `tikv-jemallocator` / `mimalloc` | Allocators globales alternativos (opcionales) |
| `tokio` | Ingesta async TCP (opcional) |
| `tokio-uring` | Ingesta io_uring (opcional) |
| `cudarc` | Kernels CUDA y alloc VRAM (opcional) |

## Interpretación rápida de métricas

| Métrica | Fuente | Qué indica |
|---------|--------|------------|
| p50 / p99 / p99.9 | HdrHistogram (app) | Latencia percibida por el workload |
| Throughput | Wall-clock (app) | Payloads procesados por segundo |
| cycles / instructions | perf (hardware) | Coste computacional total |
| cache-misses | perf (hardware) | Presión sobre la jerarquía de caché |
| page-faults | perf (kernel) | Crecimiento del heap / mapeo de memoria |
| context-switches | perf (kernel) | Contención del planificador (sube en cargas largas) |

## Licencia

MIT
