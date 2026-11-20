#!/bin/bash
# Сборка выпуска лаунчера для Windows — только на Windows (Git Bash): build.rs GPUI
# (gpui-pre-windows) компилирует шейдеры fxc.exe из Windows SDK, кросс-сборка с Linux
# (cargo-xwin) их не соберёт. Нужны Visual Studio Build Tools (MSVC и Windows SDK) и python.
#
#   scripts/slint/build-windows.sh [--package] [аргументы cargo]
#
# Итог — target/dist/windows/HLSR.exe и hlsr.pdb рядом; проверка ресурсов. С --package после сборки
# hlsr-release упаковывает установщик HLSR-Setup-<версия>.exe и latest.yml (package-windows.sh;
# версия выпуска - ровно X.Y.Z). Подпись канала и публикация - отдельно, только с ключом владельца.
# Последний шаг, fat LTO hlsr.exe, берёт ~8 ГБ ОЗУ. Если столько нет, конвейер проверяется на тех же
# зависимостях (с теми же RUSTFLAGS) так: cargo rustc -p hlsr-app --release --bin hlsr -- -C lto=thin.
set -euo pipefail

case "$(uname -s)" in
    MINGW* | MSYS* | CYGWIN*) ;;
    *)
        echo "build-windows.sh: только Windows (Git Bash): шейдеры GPUI собирает fxc.exe из Windows SDK" >&2
        exit 2
        ;;
esac

repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"
package=0
cargo_args=()
for arg in "$@"; do
    if [ "$arg" = "--package" ]; then package=1; else cargo_args+=("$arg"); fi
done
# Пути сборки в exe — нейтральные (/cargo/…, /hlsr/…; remap-flags.sh), как в release-windows.yml.
RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }$("$repo/scripts/slint/remap-flags.sh")"
export RUSTFLAGS
cargo build -p hlsr-app --release ${cargo_args[@]+"${cargo_args[@]}"}

dist=$repo/target/dist/windows
mkdir -p "$dist"
cp target/release/hlsr.exe "$dist/HLSR.exe"
cp target/release/hlsr.pdb "$dist/hlsr.pdb"
ls -l "$dist/HLSR.exe" "$dist/hlsr.pdb" | awk '{printf "%s %.1f МБ\n", $NF, $5 / 1048576}'

# Ресурсы exe (build.rs hlsr-app): типы верхнего уровня каталога .rsrc PE32+.
PYTHONIOENCODING=utf-8 python - "$(cygpath -w "$dist/HLSR.exe")" <<'EOF'
import struct, sys

data = open(sys.argv[1], "rb").read()
pe = struct.unpack_from("<I", data, 0x3C)[0]
sections, optional_size = struct.unpack_from("<H12xH", data, pe + 6)
optional = pe + 24
rsrc_rva = struct.unpack_from("<I", data, optional + 112 + 2 * 8)[0]
table = optional + optional_size
for i in range(sections):
    vsize, va, _, raw = struct.unpack_from("<4I", data, table + 40 * i + 8)
    if va <= rsrc_rva < va + vsize:
        root = rsrc_rva - va + raw
        break
else:
    sys.exit("ресурсов нет")
named, ids = struct.unpack_from("<HH", data, root + 12)
types = {struct.unpack_from("<I", data, root + 16 + 8 * (named + i))[0] for i in range(ids)}
missing = 0
for kind, number in (("ICON", 3), ("GROUP_ICON", 14), ("VERSIONINFO", 16), ("MANIFEST", 24)):
    print(f"ресурс {kind}: {'есть' if number in types else 'НЕТ'}")
    missing |= number not in types
sys.exit(missing)
EOF
if [ "$package" = 1 ]; then
    "$repo/scripts/slint/package-windows.sh" --exe "$dist/HLSR.exe"
fi
