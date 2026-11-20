#!/bin/bash
# Сборка лаунчера для Linux в контейнере Ubuntu 18.04 (glibc 2.27).
#
#   scripts/slint/build-linux.sh [--release] [--package]
#
# Итог — target/dist/linux/hlsr (без отладочных символов, со ссылкой
# .gnu_debuglink) и hlsr.debug рядом; затем проверка glibc. С --package (только вместе с --release)
# после сборки hlsr-release упаковывает архив лаунчера, payload.json, AppImage и latest-linux.yml
# (package-linux.sh: нужны mksquashfs и закреплённый рантайм AppImage; версия выпуска - ровно X.Y.Z).
set -euo pipefail

repo=$(cd "$(dirname "$0")/../.." && pwd)
profile=debug
flags=()
package=0
for arg in "$@"; do
    case "$arg" in
        --release) profile=release; flags=(--release) ;;
        --package) package=1 ;;
        *) echo "build-linux.sh: неизвестный аргумент $arg" >&2; exit 2 ;;
    esac
done
if [ "$package" = 1 ] && [ "$profile" != release ]; then
    echo "build-linux.sh: --package только вместе с --release" >&2
    exit 2
fi
image=hlsr-build:18.04

# Rootless podman в сеансе без прав на newuidmap работает только из
# пользовательского менеджера systemd; в CI и в обычной оболочке — напрямую.
run_podman() {
    if podman info >/dev/null 2>&1; then
        podman "$@"
    else
        systemd-run --user --wait --pipe --quiet --same-dir podman "$@"
    fi
}

# Сеть хоста: в сети rootless podman по умолчанию не работает DNS хоста
# (systemd-resolved на 127.0.0.53). Прокси хоста в контейнер не передаётся.
net=(--network=host --http-proxy=false)
# Пути сборки в бинарнике — нейтральные: /opt/cargo (CARGO_HOME контейнера) → /cargo, /src → /hlsr.
rustflags=$("$repo/scripts/slint/remap-flags.sh" --cargo-home /opt/cargo --root /src)
run_podman build "${net[@]}" -t "$image" -f "$repo/packaging/linux/Containerfile" "$repo/packaging/linux"
run_podman run --rm "${net[@]}" \
    -v "$repo:/src" \
    -v hlsr-cargo-registry:/opt/cargo/registry \
    -w /src \
    -e CARGO_TARGET_DIR=/src/target/linux-1804 \
    -e RUSTFLAGS="$rustflags" \
    "$image" cargo build -p hlsr-app --locked "${flags[@]}"

dist=$repo/target/dist/linux
mkdir -p "$dist"
cp "$repo/target/linux-1804/$profile/hlsr" "$dist/hlsr"
cd "$dist"
objcopy --only-keep-debug hlsr hlsr.debug
objcopy --strip-debug --add-gnu-debuglink=hlsr.debug hlsr
ls -l hlsr hlsr.debug | awk '{printf "%s %.1f МБ\n", $NF, $5 / 1048576}'
"$repo/scripts/slint/check-glibc.sh" hlsr
if [ "$package" = 1 ]; then
    "$repo/scripts/slint/package-linux.sh" --exe "$dist/hlsr"
fi
