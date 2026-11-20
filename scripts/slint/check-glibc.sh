#!/bin/bash
# Наибольшая версия GLIBC_x.y, которую требует бинарник. Больше 2.27 — код 1
# и список таких символов.
#
#   scripts/slint/check-glibc.sh <бинарник>
set -euo pipefail

bin=$1
limit=2.27
# Строки objdump -T: «… DF *UND* … (GLIBC_2.2.5) memcpy».
symbols=$(objdump -T "$bin" | sed -nE 's/.*\(GLIBC_([0-9.]+)\)[[:space:]]+([^[:space:]]+)$/\1 \2/p')
max=$(printf '%s\n' "$symbols" | cut -d' ' -f1 | grep . | sort -uV | tail -1 || true)
echo "наибольшая версия GLIBC: ${max:-нет}"

newer() { [ "$(printf '%s\n%s\n' "$1" "$limit" | sort -V | tail -1)" != "$limit" ]; }

if [ -n "$max" ] && newer "$max"; then
    echo "GLIBC $limit или старше: нет — символы новее $limit:"
    printf '%s\n' "$symbols" | while read -r version name; do
        if newer "$version"; then echo "  $name (GLIBC_$version)"; fi
    done
    exit 1
fi
echo "GLIBC $limit или старше: да"
