#!/usr/bin/env bash
# Campaña larga de benchmarks: múltiples escenarios, repetición estadística y
# saturación de CPU para observar contadores perf y colas de latencia (p99.9).
#
# Diseño (máquina de referencia: 24 hilos lógicos, Intel Core Ultra 9 275HX):
#
#   Escenario A — JSON 2 KiB, 10M iteraciones (~15–40 s/allocator)
#   Escenario B — Borsh 2 KiB, 10M iteraciones
#   Escenario C — JSON 8 KiB, 2M iteraciones (payload grande, menos iter)
#
# Por escenario:
#   - 3 allocators (system, jemalloc, mimalloc)
#   - 3 repeticiones perf (--runs 3) → promedios en tabla
#   - 24 threads, queue-depth 96 (backpressure realista)
#   - 5 s cooldown entre allocators, 15 s entre escenarios
#
# Tiempo estimado total: 25–45 minutos
#
# Uso:
#   ./scripts/run_benchmarks_long.sh
#   ./scripts/run_benchmarks_long.sh --quick   # versión reducida para prueba

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

THREADS="$(nproc)"
QUEUE_DEPTH=$((THREADS * 4))
PERF_RUNS=3
COOLDOWN_ALLOC=5
COOLDOWN_SCENARIO=15
CAMPAIGN_DIR="$ROOT/target/perf-results/long_$(date +%Y%m%d_%H%M%S)"
MANIFEST="$CAMPAIGN_DIR/manifest.txt"

# Escenarios: nombre | formato | payload_bytes | iteraciones
SCENARIOS=(
    "A_json_2k|json-like|2048|10000000"
    "B_borsh_2k|borsh-like|2048|10000000"
    "C_json_8k|json-like|8192|2000000"
)

QUICK_SCENARIOS=(
    "A_json_2k|json-like|2048|1000000"
    "B_borsh_2k|borsh-like|2048|1000000"
)

usage() {
    cat <<EOF
Uso: run_benchmarks_long.sh [opciones]

Opciones:
  -h, --help     Ayuda
  --quick        Campaña reducida (1M iter × 2 escenarios, 1 perf run)
  --runs N       Repeticiones perf por allocator (default: $PERF_RUNS)
  --no-build     No recompilar tras el primer escenario

Variables de entorno:
  THREADS        Hilos worker (default: nproc = $THREADS)
  COOLDOWN_ALLOC Segundos entre allocators (default: $COOLDOWN_ALLOC)
EOF
}

DO_BUILD=1
QUICK=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        -h | --help)
            usage
            exit 0
            ;;
        --quick)
            QUICK=1
            PERF_RUNS=1
            COOLDOWN_ALLOC=2
            COOLDOWN_SCENARIO=5
            shift
            ;;
        --runs)
            PERF_RUNS="${2:?--runs requiere número}"
            shift 2
            ;;
        --no-build)
            DO_BUILD=0
            shift
            ;;
        *)
            echo "error: argumento desconocido: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

if [[ "$QUICK" -eq 1 ]]; then
    SCENARIOS=("${QUICK_SCENARIOS[@]}")
fi

mkdir -p "$CAMPAIGN_DIR"

{
    echo "Campaña larga — allocator benchmarks"
    echo "Inicio: $(date -Iseconds)"
    echo "Host:   $(uname -n) / $(uname -r)"
    echo "CPU:    ${THREADS} hilos lógicos"
    echo "Perf:   ${PERF_RUNS} repeticiones por allocator"
    echo "Salida: $CAMPAIGN_DIR"
    echo
    echo "Escenarios:"
    for scenario in "${SCENARIOS[@]}"; do
        IFS='|' read -r name format payload iterations <<< "$scenario"
        echo "  - $name: format=$format payload=${payload}B iterations=$iterations"
    done
} | tee "$MANIFEST"

echo
echo "=== Iniciando campaña larga ==="
echo "Logs en: $CAMPAIGN_DIR"
echo

scenario_idx=0
for scenario in "${SCENARIOS[@]}"; do
    scenario_idx=$((scenario_idx + 1))
    IFS='|' read -r name format payload iterations <<< "$scenario"

    echo
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo " Escenario ${scenario_idx}/${#SCENARIOS[@]}: $name"
    echo " format=$format  payload=${payload}B  iterations=$iterations"
    echo " threads=$THREADS  queue-depth=$QUEUE_DEPTH  perf-runs=$PERF_RUNS"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    scenario_log="$CAMPAIGN_DIR/${name}_summary.txt"
    build_flag=""
    if [[ "$DO_BUILD" -eq 0 && "$scenario_idx" -gt 1 ]]; then
        build_flag="--no-build"
    fi

    export COOLDOWN_ALLOC="$COOLDOWN_ALLOC"

    "$ROOT/scripts/run_benchmarks.sh" \
        $build_flag \
        --runs "$PERF_RUNS" \
        -- \
        -n "$iterations" \
        -t "$THREADS" \
        -s "$payload" \
        -f "$format" \
        --queue-depth "$QUEUE_DEPTH" \
        --verbose \
        2>&1 | tee "$scenario_log"

    # Copiar el comparison más reciente al directorio de campaña
    latest_comparison="$(ls -t "$ROOT/target/perf-results"/comparison_*.txt 2>/dev/null | head -1 || true)"
    if [[ -n "$latest_comparison" ]]; then
        cp "$latest_comparison" "$CAMPAIGN_DIR/${name}_comparison.txt"
    fi

    if [[ "$scenario_idx" -lt "${#SCENARIOS[@]}" ]]; then
        echo "==> Cooldown ${COOLDOWN_SCENARIO}s antes del siguiente escenario..."
        sleep "$COOLDOWN_SCENARIO"
    fi
done

{
    echo
    echo "Campaña finalizada: $(date -Iseconds)"
    echo "Resúmenes por escenario:"
    for scenario in "${SCENARIOS[@]}"; do
        IFS='|' read -r name _ _ _ <<< "$scenario"
        echo "  $CAMPAIGN_DIR/${name}_comparison.txt"
    done
} | tee -a "$MANIFEST"

echo
echo "=== Campaña completa ==="
echo "Manifiesto: $MANIFEST"
echo "Resultados: $CAMPAIGN_DIR/"
