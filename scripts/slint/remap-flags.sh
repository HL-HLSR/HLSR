#!/bin/bash
# Ключи rustc для сборки выпуска: абсолютные пути машины сборки в бинарнике (места паник, отладочные
# сведения) заменяются нейтральными — исходники зависимостей из CARGO_HOME становятся /cargo/…,
# корень репозитория (и в нём target/ с OUT_DIR сгенерированного кода) — /hlsr/…. Без них в exe
# остаются C:\Users\<пользователь>\.cargo\registry\src\… и каталог worktree.
#
#   scripts/slint/remap-flags.sh [--cargo-home <каталог>] [--root <каталог>] [--target-dir <каталог>]
#
# Печатает ключи через пробел (для RUSTFLAGS или CARGO_TARGET_<цель>_RUSTFLAGS). По умолчанию:
# CARGO_HOME (иначе ~/.cargo), корень этого репозитория, CARGO_TARGET_DIR (иначе <корень>/target).
# Пути — в виде, который видит rustc (на Windows — C:\…); каталоги из ключей (контейнер сборки) — как
# есть. Члены workspace cargo и так передаёт путями от корня (crates/hlsr-…): их ремап не трогает.
set -euo pipefail

# Git Bash: /c/… → C:\… (так пути видят cargo и rustc).
native() {
    if command -v cygpath >/dev/null 2>&1; then cygpath -w "$1"; else printf '%s\n' "$1"; fi
}

repo=$(cd "$(dirname "$0")/../.." && pwd)
cargo_home=$(native "${CARGO_HOME:-$HOME/.cargo}")
root=$(native "$repo")
target_dir=""
if [ -n "${CARGO_TARGET_DIR:-}" ]; then target_dir=$(native "$CARGO_TARGET_DIR"); fi
while [ $# -gt 0 ]; do
    case "$1" in
        --cargo-home) cargo_home=$2; shift 2 ;;
        --root) root=$2; target_dir=""; shift 2 ;;
        --target-dir) target_dir=$2; shift 2 ;;
        *) echo "remap-flags.sh: неизвестный аргумент $1" >&2; exit 2 ;;
    esac
done
target_dir=${target_dir:-$root/target}

for path in "$cargo_home" "$root" "$target_dir"; do
    case "$path" in
        *[[:space:]]*) echo "remap-flags.sh: путь с пробелом не уместится в RUSTFLAGS: $path" >&2; exit 1 ;;
    esac
done

# rustc берёт последнее подходящее правило: target/ вне репозитория — после корня.
flags="--remap-path-prefix=$cargo_home=/cargo --remap-path-prefix=$root=/hlsr"
case "$target_dir" in
    "$root" | "$root"[\\/]*) ;;
    *) flags="$flags --remap-path-prefix=$target_dir=/hlsr/target" ;;
esac
printf '%s\n' "$flags"
