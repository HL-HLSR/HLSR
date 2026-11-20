#!/bin/bash
# Приёмка 0.4.7 для Linux (ПП6, задача 8): готовые артефакты выпуска проходят проверки замороженной
# обновлялки 0.4.7 (tools/compat-047/accept-release.js): канал -> gate с локального HTTP -> вшитый
# архив достаётся НАСТОЯЩИМ рантаймом AppImage (--appimage-extract) -> prepare (размер, sha256, sha512,
# строгий USTAR, verifyStage) -> commit во временный корень. Только Linux (символические ссылки,
# /proc, рантайм AppImage). Выпуск, не прошедший приёмку, не публикуется.
#
#   scripts/slint/accept-linux.sh [--dist <каталог>] [--test-keys <json>] [--smoke]
#
#   --dist <каталог>     готовый каталог выпуска (HLSR-Setup-<v>-linux-x86_64.AppImage, latest-linux.yml,
#                        launcher-channel.json; payload.json и архив рядом - по желанию): проверяется как
#                        есть. Боевой канал - вшитые ключи 0.4.7; тестовый - с --test-keys.
#   без --dist           берёт target/dist/linux (его оставляет package-linux.sh) и подписывает канал
#                        ТЕСТОВЫМ ключом из временного каталога (боевой ключ и
#                        HLSR_MANIFEST_PRIVATE_KEY_PATH не используются); target/dist/linux не меняется.
#   --smoke              запустить <версия>/hlsr --no-sandbox под headless.sh с HLSR_SMOKE_SNAPSHOT
#                        (нужна сборка с dev-hooks; в боевой сборке снимка нет).
set -euo pipefail

repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"
dist=""
keys=""
smoke=()
while [ $# -gt 0 ]; do
    case "$1" in
        --dist) dist=$2; shift 2 ;;
        --test-keys) keys=$2; shift 2 ;;
        --smoke) smoke=(--smoke); shift ;;
        *) echo "accept-linux.sh: неизвестный аргумент $1" >&2; exit 2 ;;
    esac
done
[ "$(uname -s)" = Linux ] || { echo "accept-linux.sh: только Linux (рантайм AppImage, ссылки, /proc)" >&2; exit 2; }

accept=(node tools/compat-047/accept-release.js --platform linux)

if [ -n "$dist" ]; then
    args=("${accept[@]}" --dist "$dist")
    [ -z "$keys" ] || args+=(--test-keys "$keys")
    exec "${args[@]}" ${smoke[@]+"${smoke[@]}"}
fi

built=target/dist/linux
[ -f "$built/payload.json" ] || { echo "accept-linux.sh: нет $built/payload.json (сначала package-linux.sh)" >&2; exit 1; }
version=$(node -p "JSON.parse(require('fs').readFileSync('$built/payload.json','utf8')).version")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# Тестовые ключи во временном каталоге вне репозитория.
made=$(node tools/release/node/tests/support/make-keys.js "$work/keys" "$version" linux)
field() { node -p "JSON.parse(process.argv[1])[process.argv[2]]" "$made" "$1"; }
mkdir -p "$work/dist"
ln -s "$repo/$built/HLSR-Setup-$version-linux-x86_64.AppImage" "$work/dist/"
cp "$built/payload.json" "$built/HLSR-$version-linux-x86_64.tar.gz" "$work/dist/"
HLSR_MANIFEST_PRIVATE_KEY_PATH=$(field privateKey) HLSR_ATTEST_KEYS_DIR=$(field attestDir) \
    node tools/release/node/launcher-release-prepare-linux.js --version "$version" --build-dir "$built" \
    --out "$work/dist" --tree "$built/linux-unpacked" --chunk-store "$work/chunk-store" \
    --test-public-key "$(field publicKey)"
"${accept[@]}" --dist "$work/dist" --test-keys "$(field testKeys)" ${smoke[@]+"${smoke[@]}"}
