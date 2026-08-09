#!/usr/bin/env bash
# Ejecuta el benchmark bajo `perf stat` para cada allocator global y genera
# una tabla comparativa de contadores de hardware del kernel Linux.
#
# Uso:
#   ./scripts/run_benchmarks.sh
#   ./scripts/run_benchmarks.sh -- -n 50000 -t 4 -s 2048
#   ./scripts/run_benchmarks.sh --profile network-async -- -n 10000 -t 24
#   ./scripts/run_benchmarks.sh --ingress async-tcp --accelerator cuda-kernels
#
# Requisitos: Linux, perf (linux-tools), cargo, permisos para perf_event_open
#   sudo sysctl kernel.perf_event_paranoid=1   # o -1 para acceso completo

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

BINARY="$ROOT/target/release/allocator-benchmarks"
RESULTS_DIR="$ROOT/target/perf-results"
TIMESTAMP="$(date +%Y%m%d_%H%M%S)"
SUMMARY_FILE="$RESULTS_DIR/comparison_${TIMESTAMP}.txt"

# Argumentos base pasados al binario (después de `--`); el script añade flags de modo.
BENCH_ARGS=(-n 20000 -t 4 -s 2048 --verbose)

# Modos de ingesta y acelerador (defaults = comportamiento original)
INGRESS_MODE="${INGRESS_MODE:-in-memory}"
ACCELERATOR_MODE="${ACCELERATOR_MODE:-none}"
INGRESS_PORT="${INGRESS_PORT:-9876}"
INGRESS_CONNECTIONS="${INGRESS_CONNECTIONS:-4}"
INGRESS_READ_BATCH="${INGRESS_READ_BATCH:-64}"
CUDA_DEVICE="${CUDA_DEVICE:-0}"
GPU_BATCH_SIZE="${GPU_BATCH_SIZE:-256}"

PERF_EVENTS=(
    "cycles"
    "instructions"
    "cache-references"
    "cache-misses"
    "page-faults"
    "context-switches"
    "cpu-migrations"
)

FEATURES=(
    "use-system:system"
    "use-jemalloc:jemalloc"
    "use-mimalloc:mimalloc"
)

usage() {
    cat <<'EOF'
Uso: run_benchmarks.sh [opciones] [-- args-del-binario]

Opciones:
  -h, --help              Muestra esta ayuda
  --no-build              Omite cargo build (usa binarios existentes)
  --runs N                Repeticiones de perf stat por allocator (default: 1)
  --profile NAME          Perfil predefinido de ingesta/acelerador (ver abajo)
  --ingress MODE          in-memory | async-tcp | io-uring (default: in-memory)
  --accelerator MODE      none | cuda-kernels | cuda-serious (default: none)
  --ingress-port N        Puerto TCP para modos de red (default: 9876)
  --ingress-connections N Conexiones cliente TCP (default: 4)
  --ingress-read-batch N  Frames por batch de lectura (default: 64)
  --cuda-device N         Índice GPU CUDA (default: 0)
  --gpu-batch-size N      Tamaño batch GPU (default: 256)

Perfiles (--profile):
  baseline          in-memory + none (original)
  network-async     async-tcp + none        (feature: ingress-async)
  network-io-uring  io-uring + none           (feature: ingress-io-uring)
  gpu-kernels       in-memory + cuda-kernels  (feature: gpu-cuda)
  gpu-serious       in-memory + cuda-serious  (feature: gpu-serious)
  network-gpu       async-tcp + cuda-kernels  (ingress-async + gpu-cuda)

Ejemplos:
  ./scripts/run_benchmarks.sh -- -n 50000 -t 8 -s 4096 -f borsh-like
  ./scripts/run_benchmarks.sh --profile network-async -- -n 10000 -t 24 -s 2048
  ./scripts/run_benchmarks.sh --ingress io-uring --accelerator cuda-serious \
      --gpu-batch-size 128 -- -n 5000 -t 8
EOF
}

require_command() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "error: '$1' no encontrado en PATH" >&2
        exit 1
    fi
}

cuda_available() {
    command -v nvidia-smi >/dev/null 2>&1 && nvidia-smi >/dev/null 2>&1
}

apply_profile() {
    local profile="$1"
    case "$profile" in
        baseline)
            INGRESS_MODE=in-memory
            ACCELERATOR_MODE=none
            ;;
        network-async)
            INGRESS_MODE=async-tcp
            ACCELERATOR_MODE=none
            ;;
        network-io-uring)
            INGRESS_MODE=io-uring
            ACCELERATOR_MODE=none
            ;;
        gpu-kernels)
            INGRESS_MODE=in-memory
            ACCELERATOR_MODE=cuda-kernels
            ;;
        gpu-serious)
            INGRESS_MODE=in-memory
            ACCELERATOR_MODE=cuda-serious
            GPU_BATCH_SIZE="${GPU_BATCH_SIZE:-128}"
            ;;
        network-gpu)
            INGRESS_MODE=async-tcp
            ACCELERATOR_MODE=cuda-kernels
            ;;
        *)
            echo "error: perfil desconocido: $profile" >&2
            echo "Perfiles válidos: baseline, network-async, network-io-uring," >&2
            echo "  gpu-kernels, gpu-serious, network-gpu" >&2
            exit 1
            ;;
    esac
}

extra_cargo_features() {
    local extras=()
    case "$INGRESS_MODE" in
        async-tcp) extras+=("ingress-async") ;;
        io-uring) extras+=("ingress-io-uring") ;;
        in-memory) ;;
        *)
            echo "error: --ingress inválido: $INGRESS_MODE" >&2
            exit 1
            ;;
    esac
    case "$ACCELERATOR_MODE" in
        cuda-kernels) extras+=("gpu-cuda") ;;
        cuda-serious) extras+=("gpu-serious") ;;
        none) ;;
        *)
            echo "error: --accelerator inválido: $ACCELERATOR_MODE" >&2
            exit 1
            ;;
    esac
    if ((${#extras[@]} > 0)); then
        local IFS=,
        echo "${extras[*]}"
    fi
}

validate_runtime_modes() {
    case "$INGRESS_MODE" in
        in-memory | async-tcp | io-uring) ;;
        *)
            echo "error: ingress mode inválido: $INGRESS_MODE" >&2
            exit 1
            ;;
    esac
    case "$ACCELERATOR_MODE" in
        none | cuda-kernels | cuda-serious) ;;
        *)
            echo "error: accelerator mode inválido: $ACCELERATOR_MODE" >&2
            exit 1
            ;;
    esac

    if [[ "$ACCELERATOR_MODE" != "none" ]] && ! cuda_available; then
        echo "error: modo GPU '$ACCELERATOR_MODE' requiere NVIDIA GPU (nvidia-smi)" >&2
        exit 1
    fi

    if [[ "$ACCELERATOR_MODE" == "cuda-serious" ]] && [[ "$GPU_BATCH_SIZE" -lt 64 ]]; then
        echo "nota: cuda-serious requiere gpu-batch-size >= 64; usando 128" >&2
        GPU_BATCH_SIZE=128
    fi
}

mode_cli_args() {
    local args=(
        --ingress "$INGRESS_MODE"
        --ingress-port "$INGRESS_PORT"
        --ingress-connections "$INGRESS_CONNECTIONS"
        --ingress-read-batch "$INGRESS_READ_BATCH"
        --accelerator "$ACCELERATOR_MODE"
        --cuda-device "$CUDA_DEVICE"
        --gpu-batch-size "$GPU_BATCH_SIZE"
    )
    printf '%s\n' "${args[@]}"
}

resolve_perf() {
    if [[ -n "${PERF:-}" ]]; then
        if "$PERF" stat true >/dev/null 2>&1; then
            echo "$PERF"
            return 0
        fi
        echo "error: PERF='$PERF' no ejecuta 'perf stat' correctamente" >&2
        exit 1
    fi

    if command -v perf >/dev/null 2>&1 && perf stat true >/dev/null 2>&1; then
        command -v perf
        return 0
    fi

    local candidate
    candidate="$(find -L /usr/lib/linux-tools -maxdepth 2 -name perf -type f 2>/dev/null | sort -V | tail -1)"
    if [[ -z "$candidate" ]]; then
        candidate="$(find /usr/lib/linux-hwe-*-tools-* -maxdepth 1 -name perf -type f 2>/dev/null | sort -V | tail -1)"
    fi
    if [[ -n "$candidate" ]] && "$candidate" stat true >/dev/null 2>&1; then
        echo "nota: usando $candidate (kernel $(uname -r) sin linux-tools exacto)" >&2
        echo "$candidate"
        return 0
    fi

    cat >&2 <<EOF
error: perf stat no pudo ejecutarse.

Pop!_OS / kernel $(uname -r): el wrapper /usr/bin/perf busca
  /usr/lib/linux-tools/$(uname -r)/perf
pero ese paquete no existe en apt. Solución rápida:

  sudo ./scripts/fix-perf-wrapper.sh

O usa el binario directamente:
  PERF=/usr/lib/linux-tools/6.17.0-42-generic/perf ./scripts/run_benchmarks.sh

También verifica permisos:
  sudo sysctl kernel.perf_event_paranoid=1
EOF
    exit 1
}

check_perf() {
    :
}

DO_BUILD=1
PERF_RUNS=1
COOLDOWN_ALLOC="${COOLDOWN_ALLOC:-0}"
PROFILE=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        -h | --help)
            usage
            exit 0
            ;;
        --no-build)
            DO_BUILD=0
            shift
            ;;
        --runs)
            PERF_RUNS="${2:?--runs requiere un número}"
            shift 2
            ;;
        --profile)
            PROFILE="${2:?--profile requiere un nombre}"
            apply_profile "$PROFILE"
            shift 2
            ;;
        --ingress)
            INGRESS_MODE="${2:?--ingress requiere un modo}"
            shift 2
            ;;
        --accelerator)
            ACCELERATOR_MODE="${2:?--accelerator requiere un modo}"
            shift 2
            ;;
        --ingress-port)
            INGRESS_PORT="${2:?--ingress-port requiere un número}"
            shift 2
            ;;
        --ingress-connections)
            INGRESS_CONNECTIONS="${2:?--ingress-connections requiere un número}"
            shift 2
            ;;
        --ingress-read-batch)
            INGRESS_READ_BATCH="${2:?--ingress-read-batch requiere un número}"
            shift 2
            ;;
        --cuda-device)
            CUDA_DEVICE="${2:?--cuda-device requiere un número}"
            shift 2
            ;;
        --gpu-batch-size)
            GPU_BATCH_SIZE="${2:?--gpu-batch-size requiere un número}"
            shift 2
            ;;
        --)
            shift
            BENCH_ARGS=("$@")
            break
            ;;
        *)
            echo "error: argumento desconocido: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

require_command cargo
PERF_BIN="$(resolve_perf)"
check_perf
validate_runtime_modes

EXTRA_CARGO_FEATURES="$(extra_cargo_features)"
FULL_BENCH_ARGS=("${BENCH_ARGS[@]}")
while IFS= read -r flag; do
    FULL_BENCH_ARGS+=("$flag")
done < <(mode_cli_args)

mkdir -p "$RESULTS_DIR"

declare -A METRIC_VALUES
declare -A LATENCY_P50 LATENCY_P99 LATENCY_P999 LATENCY_THROUGHPUT LATENCY_ELAPSED

build_feature() {
    local feature="$1"
    local label="$2"
    local features="$feature"
    if [[ -n "$EXTRA_CARGO_FEATURES" ]]; then
        features="${feature},${EXTRA_CARGO_FEATURES}"
    fi
    echo "==> Compilando release: allocator=$feature extras=[$EXTRA_CARGO_FEATURES] ($label)"
    cargo build --release --no-default-features --features "$features"
}

run_perf_for_feature() {
    local feature="$1"
    local label="$2"
    local run_id="$3"
    local events_csv
    events_csv="$(IFS=,; echo "${PERF_EVENTS[*]}")"
    local log_file="$RESULTS_DIR/${label}_run${run_id}_${TIMESTAMP}.log"

    echo "==> perf stat ($label) run $run_id → $log_file [$(basename "$PERF_BIN")]"

    "$PERF_BIN" stat \
        -e "$events_csv" \
        -o "$log_file.raw" \
        -- "$BINARY" "${FULL_BENCH_ARGS[@]}" \
        | tee "$log_file.stdout"

    parse_perf_raw "$log_file.raw" "$label" "$run_id"
    parse_latency_stdout "$log_file.stdout" "$label" "$run_id"
}

parse_latency_stdout() {
    local stdout_file="$1"
    local label="$2"
    local run_id="$3"
    local prefix="${label}|${run_id}"

    local p50 p99 p999 throughput elapsed
    p50="$(grep -E '^\s+p50' "$stdout_file" | awk '{print $2}' | head -1 || true)"
    p99="$(grep -E '^\s+p99' "$stdout_file" | awk '{print $2}' | head -1 || true)"
    p999="$(grep -E '^\s+p99\.9' "$stdout_file" | awk '{print $2}' | head -1 || true)"
    throughput="$(grep -E '^Throughput:' "$stdout_file" | awk '{print $2}' | head -1 || true)"
    elapsed="$(grep -E '^Elapsed:' "$stdout_file" | awk '{print $2}' | head -1 || true)"

    [[ -n "$p50" ]] && LATENCY_P50["$prefix"]="$p50"
    [[ -n "$p99" ]] && LATENCY_P99["$prefix"]="$p99"
    [[ -n "$p999" ]] && LATENCY_P999["$prefix"]="$p999"
    [[ -n "$throughput" ]] && LATENCY_THROUGHPUT["$prefix"]="$throughput"
    [[ -n "$elapsed" ]] && LATENCY_ELAPSED["$prefix"]="$elapsed"
}

parse_perf_raw() {
    local raw_file="$1"
    local label="$2"
    local run_id="$3"

    while IFS= read -r line; do
        if [[ "$line" == *"<not counted>"* ]] || [[ "$line" =~ ^[[:space:]]*# ]]; then
            continue
        fi

        if [[ "$line" =~ ^[[:space:]]*([0-9,]+)[[:space:]]+([^[:space:]]+) ]]; then
            local value="${BASH_REMATCH[1]//,/}"
            local raw_event="${BASH_REMATCH[2]}"
            local event="$raw_event"
            if [[ "$event" == */* ]]; then
                event="${event#*/}"
                event="${event%%/*}"
            fi
            event="${event%%:*}"

            local key="${label}|${event}|${run_id}"
            METRIC_VALUES["$key"]="$value"
        fi
    done < "$raw_file"
}

metric_average() {
    local label="$1"
    local event="$2"
    local sum=0
    local count=0
    local run

    for ((run = 1; run <= PERF_RUNS; run++)); do
        local key="${label}|${event}|${run}"
        if [[ -n "${METRIC_VALUES[$key]:-}" ]]; then
            sum=$((sum + METRIC_VALUES[$key]))
            count=$((count + 1))
        fi
    done

    if [[ "$count" -eq 0 ]]; then
        echo "N/A"
    else
        echo $((sum / count))
    fi
}

latency_average() {
    local assoc_name="$1"
    local label="$2"
    local -n values="$assoc_name"
    local sum=0
    local count=0
    local run

    for ((run = 1; run <= PERF_RUNS; run++)); do
        local key="${label}|${run}"
        if [[ -n "${values[$key]:-}" ]]; then
            local num="${values[$key]%% *}"
            num="${num//,/.}"
            sum="$(awk "BEGIN { print $sum + $num }")"
            count=$((count + 1))
        fi
    done

    if [[ "$count" -eq 0 ]]; then
        echo "N/A"
    else
        awk "BEGIN { printf \"%.2f\", $sum / $count }"
    fi
}

print_comparison_table() {
    {
        echo "Allocator Benchmark — perf stat comparison"
        echo "Generated: $(date -Iseconds)"
        echo "Binary:    $BINARY"
        echo "Profile:   ${PROFILE:-custom}"
        echo "Ingress:   $INGRESS_MODE (port=$INGRESS_PORT conn=$INGRESS_CONNECTIONS batch=$INGRESS_READ_BATCH)"
        echo "Accelerator: $ACCELERATOR_MODE (device=$CUDA_DEVICE gpu-batch=$GPU_BATCH_SIZE)"
        echo "Cargo extras: ${EXTRA_CARGO_FEATURES:-none}"
        echo "Args:      ${FULL_BENCH_ARGS[*]}"
        echo "Runs:      $PERF_RUNS per allocator"
        echo
        printf "%-14s" "Metric"
        for entry in "${FEATURES[@]}"; do
            local label="${entry#*:}"
            printf " %18s" "$label"
        done
        echo
        printf "%-14s" "--------------"
        for _ in "${FEATURES[@]}"; do
            printf " %18s" "------------------"
        done
        echo

        for event in "${PERF_EVENTS[@]}"; do
            printf "%-14s" "$event"
            for entry in "${FEATURES[@]}"; do
                local label="${entry#*:}"
                local value
                value="$(metric_average "$label" "$event")"
                if [[ "$value" =~ ^[0-9]+$ ]]; then
                    printf " %18s" "$(printf "%'d" "$value" 2>/dev/null || echo "$value")"
                else
                    printf " %18s" "$value"
                fi
            done
            echo
        done

        if [[ "$PERF_RUNS" -gt 1 ]]; then
            echo
            echo "(Valores perf: promedio de $PERF_RUNS corridas)"
        fi

        echo
        echo "Latencia de aplicación (HdrHistogram):"
        printf "%-14s" "Metric"
        for entry in "${FEATURES[@]}"; do
            printf " %18s" "${entry#*:}"
        done
        echo
        for metric_name in "p50_ns:LATENCY_P50" "p99_ns:LATENCY_P99" "p99.9_ns:LATENCY_P999" "throughput/s:LATENCY_THROUGHPUT" "elapsed_s:LATENCY_ELAPSED"; do
            local row_label="${metric_name%%:*}"
            local -n store="${metric_name##*:}"
            printf "%-14s" "$row_label"
            for entry in "${FEATURES[@]}"; do
                local label="${entry#*:}"
                local avg
                avg="$(latency_average store "$label")"
                printf " %18s" "$avg"
            done
            echo
        done

        echo
        echo "Raw perf logs: $RESULTS_DIR/*_${TIMESTAMP}.log*"
    } | tee "$SUMMARY_FILE"
}

echo "=== Allocator Benchmark Suite — perf stat ==="
echo "Project: $ROOT"
echo "Ingress: $INGRESS_MODE | Accelerator: $ACCELERATOR_MODE"
echo

for entry in "${FEATURES[@]}"; do
    feature="${entry%%:*}"
    label="${entry#*:}"

    if [[ "$DO_BUILD" -eq 1 ]]; then
        build_feature "$feature" "$label"
    elif [[ ! -x "$BINARY" ]]; then
        echo "error: binario no encontrado en $BINARY (ejecuta sin --no-build)" >&2
        exit 1
    fi

    for run in $(seq 1 "$PERF_RUNS"); do
        run_perf_for_feature "$feature" "$label" "$run"
    done

    if [[ "$COOLDOWN_ALLOC" -gt 0 ]]; then
        echo "==> Cooldown ${COOLDOWN_ALLOC}s..."
        sleep "$COOLDOWN_ALLOC"
    fi
done

echo
print_comparison_table

echo
echo "Resumen guardado en: $SUMMARY_FILE"
