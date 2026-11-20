#!/bin/bash
# Упаковка выпуска Linux (ПП6, задача 8): hlsr -> архив лаунчера (строгий USTAR .tar.gz), payload.json,
# AppImage-установщик HLSR-Setup-<версия>-linux-x86_64.AppImage и latest-linux.yml.
#
#   scripts/slint/package-linux.sh [--exe <hlsr>] [--out <каталог>] [--dev]
#
# Берёт target/dist/linux/hlsr (его кладёт build-linux.sh --release) и пишет рядом:
#   linux-unpacked/          дерево сборки (hlsr + resources/...): из него Node режет куски обновления
#   payload/                 HLSR-<v>-linux-x86_64.tar.gz, <архив>.json, payload.json
#   HLSR-Setup-<v>-linux-x86_64.AppImage, latest-linux.yml
# Нужны: squashfs-tools 4.4+ (mksquashfs с zstd) и рантайм AppImage type2 с ЗАКРЕПЛЁННЫМ sha256
# (Р11). Рантайм - один файл, скачивается один раз:
#   HLSR_APPIMAGE_RUNTIME=<файл>   готовый рантайм (иначе качается в target/appimage-runtime/);
#   packaging/linux/appimage-runtime.env   адрес и sha256 (RUNTIME_URL, RUNTIME_SHA256); пока sha256
#                                          не закреплён владельцем, упаковка AppImage отказывается.
# Версия выпуска - ровно X.Y.Z (Р9); --dev берёт числа версии без -dev (пробная упаковка).
# Подпись канала и публикация - не здесь: tools/release/node/launcher-release-prepare-linux.js.
set -euo pipefail

repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"
dist=target/dist/linux
exe=""
dev=0
while [ $# -gt 0 ]; do
    case "$1" in
        --exe) exe=$2; shift 2 ;;
        --out) dist=$2; shift 2 ;;
        --dev) dev=1; shift ;;
        *) echo "package-linux.sh: неизвестный аргумент $1" >&2; exit 2 ;;
    esac
done
exe=${exe:-$dist/hlsr}
[ -f "$exe" ] || { echo "package-linux.sh: нет $exe (сначала build-linux.sh --release)" >&2; exit 1; }

# shellcheck source=/dev/null
. packaging/linux/appimage-runtime.env
runtime=${HLSR_APPIMAGE_RUNTIME:-}
if [ -z "$runtime" ]; then
    if [ "$RUNTIME_SHA256" = "UNPINNED" ]; then
        echo "package-linux.sh: sha256 рантайма AppImage не закреплён (packaging/linux/appimage-runtime.env)." >&2
        echo "Скачайте $RUNTIME_URL один раз, сверьте с релизом type2-runtime, впишите sha256sum в RUNTIME_SHA256." >&2
        exit 1
    fi
    runtime=target/appimage-runtime/runtime-x86_64
    if [ ! -f "$runtime" ]; then
        mkdir -p "$(dirname "$runtime")"
        curl --proto '=https' --tlsv1.2 -fsSL "$RUNTIME_URL" -o "$runtime.partial"
        mv "$runtime.partial" "$runtime"
    fi
    echo "$RUNTIME_SHA256  $runtime" | sha256sum -c -
fi
command -v mksquashfs >/dev/null || { echo "package-linux.sh: нужен mksquashfs (squashfs-tools 4.4+)" >&2; exit 1; }

cargo build --release -p hlsr-release
tool=target/release/hlsr-release

if [ "$dev" = 1 ]; then
    version=$("$tool" version | sed 's/-.*//')
else
    version=$("$tool" version --release)
fi
echo "упаковка Linux $version"

tree=$dist/linux-unpacked
rm -rf "$tree" "$dist/payload"
mkdir -p "$tree"
cp "$exe" "$tree/hlsr"
"$tool" stage --platform linux --repo "$repo" --out "$tree/resources"
"$tool" archive --tree "$tree" --version "$version" --out "$dist/payload"
"$tool" appimage --payload "$dist/payload" --exe "$exe" --icon build/icons/256x256.png \
    --runtime "$runtime" --runtime-sha256 "${RUNTIME_SHA256}" \
    --out "$dist/HLSR-Setup-$version-linux-x86_64.AppImage"
# Рядом с AppImage - вшитый архив и описание: приёмка 0.4.7 берёт их, если рантайм не запустить.
cp "$dist/payload/payload.json" "$dist/payload/HLSR-$version-linux-x86_64.tar.gz" "$dist/"
