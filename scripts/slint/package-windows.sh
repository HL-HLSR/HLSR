#!/bin/bash
# Упаковка выпуска Windows (ПП6, задача 8): HLSR.exe -> HLSR-Setup-<версия>.exe и latest.yml.
#
#   scripts/slint/package-windows.sh [--exe <HLSR.exe>] [--out <каталог>] [--qa] [--dev]
#
# По умолчанию берёт target/dist/windows/HLSR.exe (его кладёт build-windows.sh) и пишет рядом:
# HLSR-Setup-<v>.exe (exe + нагрузка + хвост), latest.yml, win-unpacked/ (из неё Node режет куски).
# Версия выпуска - ровно X.Y.Z (Р9): Cargo.toml с 0.5.0-dev упаковщик отвергает. --dev берёт числа
# версии без -dev (пробная упаковка; ProductVersion exe всё равно 0.5.0.0, апдейтер 0.4.7 его и сверяет).
# Окно установщика показывает версию нагрузки, как реестр; сам лаунчер (main.log, «О программе») - 0.5.0-dev.
# --qa - QA-идентичность (com.hlsrteam.hlsr.qa, свой GUID и каталог C:\HLSR-QA\Launcher, решение 18).
# Подпись канала и публикация - не здесь: tools/release/node/launcher-release-prepare.js, только с
# ключом владельца.
set -euo pipefail

repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"
dist=target/dist/windows
exe=""
dev=0
qa=()
while [ $# -gt 0 ]; do
    case "$1" in
        --exe) exe=$2; shift 2 ;;
        --out) dist=$2; shift 2 ;;
        --qa) qa=(--qa); shift ;;
        --dev) dev=1; shift ;;
        *) echo "package-windows.sh: неизвестный аргумент $1" >&2; exit 2 ;;
    esac
done
exe=${exe:-$dist/HLSR.exe}
[ -f "$exe" ] || { echo "package-windows.sh: нет $exe (сначала build-windows.sh или cargo build --release)" >&2; exit 1; }

cargo build --release -p hlsr-release
case "$(uname -s)" in
    MINGW* | MSYS* | CYGWIN*) tool=target/release/hlsr-release.exe ;;
    *) tool=target/release/hlsr-release ;;
esac

if [ "$dev" = 1 ]; then
    version=$("$tool" version | sed 's/-.*//')
else
    version=$("$tool" version --release)
fi
echo "упаковка Windows $version"

resources=target/dist/windows-resources
rm -rf "$resources"
"$tool" stage --platform windows --repo "$repo" --out "$resources"
"$tool" setup --exe "$exe" --resources "$resources" --version "$version" ${qa[@]+"${qa[@]}"} --out "$dist"
rm -rf "$resources"
