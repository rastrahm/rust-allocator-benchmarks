#!/usr/bin/env bash
# Crea el symlink que /usr/bin/perf espera para el kernel Pop!_OS en ejecución.
#
# En Pop!_OS con kernel 6.18.x, apt instala linux-tools-6.17 pero el wrapper busca:
#   /usr/lib/linux-tools/$(uname -r)/perf
#
# Uso:
#   sudo ./scripts/fix-perf-wrapper.sh

set -euo pipefail

KERNEL="$(uname -r)"
TARGET_DIR="/usr/lib/linux-tools/${KERNEL}"
TARGET_LINK="${TARGET_DIR}/perf"

if [[ "${EUID}" -ne 0 ]]; then
    echo "error: ejecuta con sudo:" >&2
    echo "  sudo $0" >&2
    exit 1
fi

# Buscar el binario perf más reciente instalado
PERF_SOURCE=""
for candidate in \
    "$(find -L /usr/lib/linux-tools -maxdepth 2 -name perf -type f 2>/dev/null | sort -V | tail -1)" \
    "$(find /usr/lib/linux-hwe-*-tools-* -maxdepth 1 -name perf -type f 2>/dev/null | sort -V | tail -1)"; do
    if [[ -n "$candidate" && -x "$candidate" ]]; then
        PERF_SOURCE="$candidate"
        break
    fi
done

if [[ -z "$PERF_SOURCE" ]]; then
    cat >&2 <<EOF
error: no se encontró ningún binario perf instalado.

Instala linux-tools primero:
  sudo apt install linux-tools-common linux-tools-virtual-6.17
EOF
    exit 1
fi

mkdir -p "$TARGET_DIR"
ln -sf "$PERF_SOURCE" "$TARGET_LINK"

echo "Kernel:   $KERNEL"
echo "Origen:   $PERF_SOURCE"
echo "Enlace:   $TARGET_LINK"
echo

if perf --version; then
    echo
    echo "perf stat de prueba:"
    perf stat true
    echo
    echo "Listo. 'perf' ya debería funcionar sin WARNING."
else
    echo "error: perf sigue fallando tras crear el enlace" >&2
    exit 1
fi
