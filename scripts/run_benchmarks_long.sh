#!/usr/bin/env bash
# Campaña larga de benchmarks: múltiples escenarios, repetición estadística y
# saturación de CPU para observar contadores perf y colas de latencia (p99.9).
#
# Diseño baseline (máquina de referencia: 24 hilos lógicos):
#
#   Escenario A — JSON 2 KiB, 10M iteraciones, in-memory
#   Escenario B — Borsh 2 KiB, 10M iteraciones, in-memory
#   Escenario C — JSON 8 KiB, 2M iteraciones, in-memory
#
# Con --extended añade escenarios de red y GPU:
#
#   D — async-tcp, JSON 2 KiB
#   E — io_uring, JSON 2 KiB
#   F — cuda-kernels, JSON 2 KiB
#   G — cuda-serious, JSON 2 KiB
#   H — async-tcp + cuda-kernels
#
# Formato de escenario:
#   nombre|formato|payload_bytes|iteraciones|ingress|accelerator
#
# Uso:
#   ./scripts/run_benchmarks_long.sh
#   ./scripts/run_benchmarks_long.sh --quick
#   ./scripts/run_benchmarks_long.sh --extended

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

# Escenarios baseline (comportamiento original)
SCENARIOS=(
    "A_json_2k|json-like|2048|10000000|in-memory|none"
    "B_borsh_2k|borsh-like|2048|10000000|in-memory|none"
    "C_json_8k|json-like|8192|2000000|in-memory|none"
)

QUICK_SCENARIOS=(
    "A_json_2k|json-like|2048|1000000|in-memory|none"
    "B_borsh_2k|borsh-like|2048|1000000|in-memory|none"
)

# Escenarios extendidos: red + GPU (iteraciones reducidas vs baseline)
EXTENDED_SCENARIOS=(
    "D_async_tcp|json-like|2048|2000000|async-tcp|none"
    "E_io_uring|json-like|2048|2000000|io-uring|none"
    "F_gpu_kernels|json-like|2048|1000000|in-memory|cuda-kernels"
    "G_gpu_serious|json-like|2048|500000|in-memory|cuda-serious"
    "H_async_gpu|json-like|2048|1000000|async-tcp|cuda-kernels"
)

QUICK_EXTENDED_SCENARIOS=(
    "D_async_tcp|json-like|2048|200000|async-tcp|none"
    "F_gpu_kernels|json-like|2048|100000|in-memory|cuda-kernels"
)

usage() {
    cat <<EOF
Uso: run_benchmarks_long.sh [opciones]

Opciones:
  -h, --help     Ayuda
  --quick        Campaña reducida (1M iter baseline, menos escenarios)
  --extended     Incluye escenarios de red (async-tcp, io-uring) y GPU
  --runs N       Repeticiones perf por allocator (default: $PERF_RUNS)
  --no-build     No recompilar tras el primer escenario de cada perfil

Variables de entorno:
  THREADS        Hilos worker (default: nproc = $THREADS)
  COOLDOWN_ALLOC Segundos entre allocators (default: $COOLDOWN_ALLOC)
  INGRESS_PORT   Puerto TCP para escenarios de red (default: 9876)
  GPU_BATCH_SIZE Batch GPU (default: 256; cuda-serious usa 128)
EOF
}

cuda_available() {
    command -v nvidia-smi >/dev/null 2>&1 && nvidia-smi >/dev/null 2>&1
}

scenario_needs_gpu() {
    local accelerator="$1"
    [[ "$accelerator" == "cuda-kernels" || "$accelerator" == "cuda-serious" ]]
}

scenario_port_offset() {
    local name="$1"
    local hash=0
    local i c
    for ((i = 0; i < ${#name}; i++)); do
        c="$(printf '%d' "'${name:$i:1}")"
        hash=$(( (hash * 31 + c) % 1000 ))
    done
    echo "$hash"
}

run_scenario() {
    local name="$1"
    local format="$2"
    local payload="$3"
    local iterations="$4"
    local ingress="$5"
    local accelerator="$6"
    local scenario_idx="$7"
    local total="$8"

    if scenario_needs_gpu "$accelerator" && ! cuda_available; then
        echo "==> Omitiendo $name: no hay GPU NVIDIA disponible" >&2
        return 0
    fi

    local port_offset
    port_offset="$(scenario_port_offset "$name")"
    local ingress_port=$((9876 + port_offset))
    local gpu_batch="$GPU_BATCH_SIZE"
    if [[ "$accelerator" == "cuda-serious" ]]; then
        gpu_batch=128
    fi

    echo
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo " Escenario ${scenario_idx}/${total}: $name"
    echo " format=$format  payload=${payload}B  iterations=$iterations"
    echo " ingress=$ingress  accelerator=$accelerator  port=$ingress_port"
    echo " threads=$THREADS  queue-depth=$QUEUE_DEPTH  perf-runs=$PERF_RUNS"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    local scenario_log="$CAMPAIGN_DIR/${name}_summary.txt"
    local build_flag=""
    if [[ "$DO_BUILD" -eq 0 && "$scenario_idx" -gt 1 ]]; then
        build_flag="--no-build"
    fi

    export COOLDOWN_ALLOC="$COOLDOWN_ALLOC"

    "$ROOT/scripts/run_benchmarks.sh" \
        $build_flag \
        --runs "$PERF_RUNS" \
        --ingress "$ingress" \
        --accelerator "$accelerator" \
        --ingress-port "$ingress_port" \
        --ingress-connections "${INGRESS_CONNECTIONS:-4}" \
        --ingress-read-batch "${INGRESS_READ_BATCH:-64}" \
        --gpu-batch-size "$gpu_batch" \
        -- \
        -n "$iterations" \
        -t "$THREADS" \
        -s "$payload" \
        -f "$format" \
        --queue-depth "$QUEUE_DEPTH" \
        --verbose \
        2>&1 | tee "$scenario_log"

    local latest_comparison
    latest_comparison="$(ls -t "$ROOT/target/perf-results"/comparison_*.txt 2>/dev/null | head -1 || true)"
    if [[ -n "$latest_comparison" ]]; then
        cp "$latest_comparison" "$CAMPAIGN_DIR/${name}_comparison.txt"
    fi

    if [[ "$scenario_idx" -lt "$total" ]]; then
        echo "==> Cooldown ${COOLDOWN_SCENARIO}s antes del siguiente escenario..."
        sleep "$COOLDOWN_SCENARIO"
    fi
}

DO_BUILD=1
QUICK=0
EXTENDED=0

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
        --extended)
            EXTENDED=1
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

ACTIVE_SCENARIOS=("${SCENARIOS[@]}")
if [[ "$QUICK" -eq 1 ]]; then
    ACTIVE_SCENARIOS=("${QUICK_SCENARIOS[@]}")
fi
if [[ "$EXTENDED" -eq 1 ]]; then
    if [[ "$QUICK" -eq 1 ]]; then
        ACTIVE_SCENARIOS+=("${QUICK_EXTENDED_SCENARIOS[@]}")
    else
        ACTIVE_SCENARIOS+=("${EXTENDED_SCENARIOS[@]}")
    fi
fi

mkdir -p "$CAMPAIGN_DIR"

{
    echo "Campaña larga — allocator benchmarks"
    echo "Inicio: $(date -Iseconds)"
    echo "Host:   $(uname -n) / $(uname -r)"
    echo "CPU:    ${THREADS} hilos lógicos"
    echo "Perf:   ${PERF_RUNS} repeticiones por allocator"
    echo "Modo:   quick=$QUICK extended=$EXTENDED"
    echo "GPU:    $(cuda_available && echo disponible || echo no detectada)"
    echo "Salida: $CAMPAIGN_DIR"
    echo
    echo "Escenarios:"
    for scenario in "${ACTIVE_SCENARIOS[@]}"; do
        IFS='|' read -r name format payload iterations ingress accelerator <<< "$scenario"
        echo "  - $name: format=$format payload=${payload}B iterations=$iterations ingress=$ingress accelerator=$accelerator"
    done
} | tee "$MANIFEST"

echo
echo "=== Iniciando campaña larga ==="
echo "Logs en: $CAMPAIGN_DIR"
echo

scenario_idx=0
total="${#ACTIVE_SCENARIOS[@]}"
for scenario in "${ACTIVE_SCENARIOS[@]}"; do
    scenario_idx=$((scenario_idx + 1))
    IFS='|' read -r name format payload iterations ingress accelerator <<< "$scenario"
    run_scenario "$name" "$format" "$payload" "$iterations" "$ingress" "$accelerator" "$scenario_idx" "$total"
done

{
    echo
    echo "Campaña finalizada: $(date -Iseconds)"
    echo "Resúmenes por escenario:"
    for scenario in "${ACTIVE_SCENARIOS[@]}"; do
        IFS='|' read -r name _ _ _ _ _ <<< "$scenario"
        echo "  $CAMPAIGN_DIR/${name}_comparison.txt"
    done
} | tee -a "$MANIFEST"

echo
echo "=== Campaña completa ==="
echo "Manifiesto: $MANIFEST"
echo "Resultados: $CAMPAIGN_DIR/"
