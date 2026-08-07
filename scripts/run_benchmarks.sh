#!/usr/bin/env bash
# Ejecuta el benchmark bajo `perf stat` para cada allocator global y genera
# una tabla comparativa de contadores de hardware del kernel Linux.
#
# Uso:
#   ./scripts/run_benchmarks.sh
#   ./scripts/run_benchmarks.sh -- -n 50000 -t 4 -s 2048
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

# Argumentos pasados al binario (después de `--`)
BENCH_ARGS=(-n 20000 -t 4 -s 2048 --verbose)

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
  -h, --help     Muestra esta ayuda
  --no-build     Omite cargo build (usa binarios existentes)
  --runs N       Repeticiones de perf stat por allocator (default: 1)

Ejemplo:
  ./scripts/run_benchmarks.sh -- -n 50000 -t 8 -s 4096 -f borsh-like
EOF
}

require_command() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "error: '$1' no encontrado en PATH" >&2
        exit 1
    fi
}

resolve_perf() {
    # Permite override manual: PERF=/ruta/a/perf ./scripts/run_benchmarks.sh
    if [[ -n "${PERF:-}" ]]; then
        if "$PERF" stat true >/dev/null 2>&1; then
            echo "$PERF"
            return 0
        fi
        echo "error: PERF='$PERF' no ejecuta 'perf stat' correctamente" >&2
        exit 1
    fi

    # Wrapper de /usr/bin/perf (falla en Pop!_OS si no hay tools para el kernel exacto)
    if command -v perf >/dev/null 2>&1 && perf stat true >/dev/null 2>&1; then
        command -v perf
        return 0
    fi

    # Fallback: binario versionado (sigue symlinks de linux-tools → linux-hwe-*-tools)
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

  sudo mkdir -p /usr/lib/linux-tools/$(uname -r)
  sudo ln -sf /usr/lib/linux-tools/6.17.0-42-generic/perf \\
       /usr/lib/linux-tools/$(uname -r)/perf

O usa el binario directamente:
  PERF=/usr/lib/linux-tools/6.17.0-42-generic/perf ./scripts/run_benchmarks.sh

También verifica permisos:
  sudo sysctl kernel.perf_event_paranoid=1
EOF
    exit 1
}

check_perf() {
    : # resuelto en resolve_perf
}

DO_BUILD=1
PERF_RUNS=1
COOLDOWN_ALLOC="${COOLDOWN_ALLOC:-0}"

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

mkdir -p "$RESULTS_DIR"

declare -A METRIC_VALUES
declare -A LATENCY_P50 LATENCY_P99 LATENCY_P999 LATENCY_THROUGHPUT LATENCY_ELAPSED

build_feature() {
    local feature="$1"
    local label="$2"
    echo "==> Compilando release con feature: $feature ($label)"
    cargo build --release --no-default-features --features "$feature"
}

run_perf_for_feature() {
    local feature="$1"
    local label="$2"
    local run_id="$3"
    local events_csv
    events_csv="$(IFS=,; echo "${PERF_EVENTS[*]}")"
    local log_file="$RESULTS_DIR/${label}_run${run_id}_${TIMESTAMP}.log"

    echo "==> perf stat ($label) run $run_id → $log_file [$(basename "$PERF_BIN")]"

    # perf escribe métricas en stderr; el binario imprime resultados en stdout.
    "$PERF_BIN" stat \
        -e "$events_csv" \
        -o "$log_file.raw" \
        -- "$BINARY" "${BENCH_ARGS[@]}" \
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
        # Ignorar líneas sin conteo o comentarios
        if [[ "$line" == *"<not counted>"* ]] || [[ "$line" =~ ^[[:space:]]*# ]]; then
            continue
        fi

        # Formato: "     1,234,567  cache-misses" o "     1,234,567  cpu_atom/cycles/u"
        if [[ "$line" =~ ^[[:space:]]*([0-9,]+)[[:space:]]+([^[:space:]]+) ]]; then
            local value="${BASH_REMATCH[1]//,/}"
            local raw_event="${BASH_REMATCH[2]}"
            local event="$raw_event"
            if [[ "$event" == */* ]]; then
                event="${event#*/}"   # cpu_atom/cycles/u → cycles/u
                event="${event%%/*}"  # cycles/u → cycles
            fi
            event="${event%%:*}"       # page-faults:u → page-faults

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
            # Valores como "4.72 µs" o "725484" — extraer número
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
        echo "Args:      ${BENCH_ARGS[*]}"
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
