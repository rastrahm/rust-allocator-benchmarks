# allocator-benchmarks

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
- Linux (para `perf stat`)
- `perf` — paquete `linux-tools` de tu kernel

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
```

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
| `--verbose` | Muestra el allocator activo en stderr | false |

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
```

### Salida

El binario imprime throughput, percentiles de latencia (p50, p90, p99, p99.9) y estadísticas básicas (min, max, mean).

## Arquitectura

```
┌─────────────┐     bounded channel     ┌──────────────┐
│  Productor  │ ──────────────────────► │   Workers    │
│  (1 hilo)   │   crossbeam-channel     │  (N hilos)   │
└─────────────┘                         └──────┬───────┘
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
| `src/main.rs` | CLI y binding del allocator global |
| `benches/` | Micro-benchmarks Criterion |

## Tests

```bash
cargo test
cargo clippy -- -D warnings
```

Incluye tests unitarios (`metrics`, `workload`, `runner`) e integración end-to-end en `tests/benchmark_integration.rs`.

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
```

Genera una tabla comparativa con contadores de hardware:

- `cycles`, `instructions`
- `cache-references`, `cache-misses`
- `page-faults`, `context-switches`, `cpu-migrations`

Resultados en `target/perf-results/comparison_<timestamp>.txt`.

### Campaña larga (recomendada para análisis serio)

```bash
./scripts/run_benchmarks_long.sh
./scripts/run_benchmarks_long.sh --quick   # versión reducida (~5 min)
```

Ejecuta tres escenarios con 24 threads, 3 repeticiones por allocator y cooldown entre corridas:

| Escenario | Formato | Payload | Iteraciones |
|-----------|---------|---------|-------------|
| A | JSON | 2 KiB | 10 M |
| B | Borsh | 2 KiB | 10 M |
| C | JSON | 8 KiB | 2 M |

Salida en `target/perf-results/long_<timestamp>/`.

## Scripts

| Script | Descripción |
|--------|-------------|
| `scripts/run_benchmarks.sh` | Compara los 3 allocators bajo `perf stat` |
| `scripts/run_benchmarks_long.sh` | Campaña multi-escenario con repetición estadística |
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
