#!/bin/bash
# Сквозная проверка перехода <база> -> 0.5.x на QA-сборках (ПП6, задача 8; карта §3.4 п. 7, спека
# "Сквозная проверка с QA-сборкой 0.4.7"; GPUI U-d: база 0.4.9). ТОЛЬКО Linux-машина или одноразовый
# раннер CI. На машине владельца, где стоит настоящий лаунчер, НЕ запускать: QA-сборки ставятся под свой GUID
# (com.hlsrteam.hlsr.qa) и в свой каталог (C:\HLSR-QA\Launcher), но пишут в реестр, ярлыки и профиль.
# Ничего не публикует: канал подписан тестовым ключом из временного каталога, лента - 127.0.0.1.
#
#   HLSR_ELECTRON_COPY=<копия эталона базы с node_modules> [HLSR_QA_E2E_BASE=0.4.9] \
#   [HLSR_QA_E2E_DRIVER=<команда>] tools/compat-047/qa-e2e.sh <windows|linux> [--keep]
#
# База (версия у пользователя, HLSR_QA_E2E_BASE) — 0.4.9 по умолчанию; 0.4.7 или 0.4.8 — копия своей
# версии. Нужно (вне автономной работы): копия эталона базы (не сам ~/HLSR-electron*: сборка пишет в
# build/; для 0.4.9 — git worktree v0.4.9, Electron 45, npm ci --legacy-peer-deps) с electron-builder
# (интернет) и, на Linux, Wine для байткода Windows; собранный 0.5.0 (dev-hooks) -
# сам скрипт собирает его через tools/release/node/qa-launcher-build.js (Windows) или
# scripts/slint/build-linux.sh --release + package-linux.sh (Linux).
#
# Шаги:
#  1. QA-сборка базы из копии эталона: HLSR_BUILD_CHANNEL=qa (feed и ключ из окружения, у 0.4.9 и
#     QA_TRUST апдейтера, работают только в такой сборке); Windows: scripts/qa-launcher-build.js
#     --version <база>, Linux: npm run build:linux.
#  2. QA-сборка 0.5.0 со своей идентичностью, канал подписан тестовым ключом (--qa-profile).
#  3. Приёмка базы над артефактами 0.5.x (accept-release.js --base <база> --expect-qa --test-keys).
#  4. Лента qa-launcher-feed.js на 127.0.0.1; в окружение старого лаунчера - HLSR_UPDATE_FEED и
#     HLSR_UPDATE_CHANNEL_KEY (PEM, переводы строк - "\n" буквами).
#  5. Старый лаунчер (база) ставится и запускается; обновление до 0.5.0 проходит путями А, Б (Windows)
#     или внутренней службой (Linux). Нажатия в окне базы делает HLSR_QA_E2E_DRIVER (команда
#     автоматизации окна на CI или человек); без драйвера скрипт печатает окружение и останавливается
#     с кодом 3 - проверка итога (шаг 6) не выполняется вслепую.
#  6. Итог: установлена 0.5.0 (ProductVersion / current), записи реестра под QA-GUID, квитанция receipt
#     подтверждена, следы Electron убраны (Р12), версии/<база>-* и Local Storage на месте.
set -euo pipefail

platform=${1:-}
keep=0
[ "${2:-}" = "--keep" ] && keep=1
case "$platform" in windows | linux) ;; *) echo "qa-e2e.sh: windows или linux" >&2; exit 2 ;; esac
repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"
: "${HLSR_ELECTRON_COPY:?нужна копия эталона базы с node_modules (HLSR_ELECTRON_COPY)}"
base=${HLSR_QA_E2E_BASE:-0.4.9}
copy_version=$(node -p "JSON.parse(require('fs').readFileSync(process.argv[1], 'utf8')).version" "$HLSR_ELECTRON_COPY/package.json")
[ "$copy_version" = "$base" ] || { echo "qa-e2e.sh: копия эталона версии $copy_version, база $base" >&2; exit 2; }
if [ "$platform" = windows ] && [ -z "${GITHUB_ACTIONS:-}" ] && [ -z "${HLSR_QA_E2E_THROWAWAY:-}" ]; then
    echo "qa-e2e.sh: Windows - только одноразовый раннер (GITHUB_ACTIONS) или HLSR_QA_E2E_THROWAWAY=1 на чистой ВМ" >&2
    exit 2
fi

work=$(mktemp -d)
feed_pid=""
cleanup() {
    [ -z "$feed_pid" ] || kill "$feed_pid" 2>/dev/null || true
    [ "$keep" = 1 ] || rm -rf "$work"
}
trap cleanup EXIT
echo "== work: $work"

version=$(node -p "require('./tools/release/node/lib/release-common').workspaceVersion().replace(/-.*/, '')")
echo "== QA 0.5.0 = $version"

echo "== 1. QA $base"
if [ "$platform" = windows ]; then
    (cd "$HLSR_ELECTRON_COPY" && node scripts/qa-launcher-build.js --version "$base" --out-root "$work/old")
    old_installer=$(ls "$work/old/$base/HLSR-Setup-$base.exe")
else
    (cd "$HLSR_ELECTRON_COPY" && HLSR_BUILD_CHANNEL=qa npm run build:linux)
    old_installer=$(ls "$HLSR_ELECTRON_COPY/build/HLSR-Setup-$base-linux-x86_64.AppImage")
fi

echo "== 2. QA 0.5.0"
made=$(node tools/release/node/tests/support/make-keys.js "$work/keys" "$version" win32 linux)
field() { node -p "JSON.parse(process.argv[1])[process.argv[2]]" "$made" "$1"; }
if [ "$platform" = windows ]; then
    node tools/release/node/qa-launcher-build.js --version "$version" --out-root "$work/new"
    dist="$work/new/$version"
    HLSR_MANIFEST_PRIVATE_KEY_PATH=$(field privateKey) HLSR_ATTEST_KEYS_DIR=$(field attestDir) \
        node tools/release/node/launcher-release-prepare.js --qa-profile --version "$version" \
        --build-dir "$dist" --out "$dist" --chunk-store "$work/chunk-store" --test-public-key "$(field publicKey)"
else
    scripts/slint/build-linux.sh --release
    scripts/slint/package-linux.sh --exe target/dist/linux/hlsr --dev
    dist=target/dist/linux
    HLSR_MANIFEST_PRIVATE_KEY_PATH=$(field privateKey) HLSR_ATTEST_KEYS_DIR=$(field attestDir) \
        node tools/release/node/launcher-release-prepare-linux.js --version "$version" --build-dir "$dist" \
        --out "$work/new" --tree "$dist/linux-unpacked" --chunk-store "$work/chunk-store" --test-public-key "$(field publicKey)"
    cp "$dist/HLSR-Setup-$version-linux-x86_64.AppImage" "$dist/payload.json" "$dist/HLSR-$version-linux-x86_64.tar.gz" "$work/new/"
    dist="$work/new"
fi

echo "== 3. приёмка базы $base"
accept_platform=$([ "$platform" = windows ] && echo win32 || echo linux)
node tools/compat-047/accept-release.js --dist "$dist" --platform "$accept_platform" --base "$base" \
    --test-keys "$(field testKeys)" $([ "$platform" = windows ] && echo --expect-qa)

echo "== 4. лента"
node tools/release/node/qa-launcher-feed.js --dir "$dist" --channel "$dist/launcher-channel.json" > "$work/feed.log" &
feed_pid=$!
for _ in $(seq 1 50); do grep -q '^FEED ' "$work/feed.log" && break; sleep 0.2; done
feed=$(sed -n 's/^FEED //p' "$work/feed.log")
[ -n "$feed" ] || { echo "лента не стартовала" >&2; exit 1; }
key_env=$(node -p "require('fs').readFileSync(process.argv[1],'utf8').replace(/\r?\n/g, '\\\\n')" "$(field publicKey)")
export HLSR_UPDATE_FEED=$feed
export HLSR_UPDATE_CHANNEL_KEY=$key_env
echo "HLSR_UPDATE_FEED=$HLSR_UPDATE_FEED"
echo "старый установщик: $old_installer"

echo "== 5. переход $base -> $version"
if [ -z "${HLSR_QA_E2E_DRIVER:-}" ]; then
    echo "HLSR_QA_E2E_DRIVER не задан: установите $old_installer, запустите его с окружением выше и" >&2
    echo "пройдите путь обновления; затем проверьте шаг 6 вручную (--keep оставляет $work)." >&2
    keep=1
    exit 3
fi
"$HLSR_QA_E2E_DRIVER" "$platform" "$old_installer" "$dist" "$feed"

echo "== 6. итог"
echo "драйвер вернул 0: проверка итога (версия, реестр QA-GUID, receipt, уборка по Р12) выполняется драйвером"
