#!/usr/bin/env bash
# Очередь на тяжёлые команды (cargo, Wine, Electron) для всех копий репозитория и всех агентов:
# одновременно выполняется не больше HLSR_SERIAL_SLOTS команд, с ограничением потоков. Каталог
# сборки — target/ своей копии (основной или worktree).
#   scripts/slint/serial.sh cargo test -p hlsr-delivery
#   scripts/slint/serial.sh scripts/slint/headless.sh cargo xwin test …
# headless.sh сам очередь не берёт: вызывать его только через serial.sh.
# Linux — flock, одно место. Windows (Git Bash, без flock) — каталоги-замки с PID владельца;
# мест по умолчанию 2, потоки делят все ядра, кроме двух (и по свободной памяти), приоритет ниже
# среднего; перед стартом ждём, пока свободной памяти станет не меньше HLSR_SERIAL_MIN_FREE_MB
# (по умолчанию 4000).
set -euo pipefail
# Свой каталог сборки у каждой копии (основной и каждого worktree): cargo называет артефакты
# членов workspace по пути относительно корня, а он у всех worktree одинаковый — общий target/
# давал чужие тестовые бинарники (собранные в другой копии, с её путями в env!) — 2026-10-19.
MAIN=$(cd "$(git -C "$(dirname "$0")" rev-parse --show-toplevel)" && pwd)
LOCK="${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}}/hlsr-serial.lock"
# rustup без правки PATH пользователя (Windows).
command -v cargo >/dev/null 2>&1 || export PATH="$HOME/.cargo/bin:$PATH"
if command -v flock >/dev/null 2>&1; then
  export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-6}"
  export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$MAIN/target}"
  exec flock "$LOCK" nice -n 10 "$@"
fi

# Потоки сборки задаются ниже, когда место и память уже есть (CARGO_BUILD_JOBS снаружи — как есть).
JOBS_SET="${CARGO_BUILD_JOBS:-}"
# Диск: без инкрементальных кешей и полной отладочной информации (worktree с разными путями крейтов
# множат артефакты; 2026-10-17 target/ вырос до 130 ГБ).
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"
export CARGO_PROFILE_DEV_DEBUG="${CARGO_PROFILE_DEV_DEBUG:-line-tables-only}"
SLOTS="${HLSR_SERIAL_SLOTS:-2}"
MIN_FREE_KB=$(( ${HLSR_SERIAL_MIN_FREE_MB:-4000} * 1024 ))
free_kb() { awk '/^MemFree:/ { print $2 }' /proc/meminfo 2>/dev/null || echo "$MIN_FREE_KB"; }

# Свободное место; брошенный замок (владелец уже не работает) снимается.
slot=""
while [ -z "$slot" ]; do
  for k in $(seq 0 $((SLOTS - 1))); do
    dir="$LOCK.$k.d"
    if mkdir "$dir" 2>/dev/null; then
      slot=$k
      break
    fi
    owner=$(cat "$dir/pid" 2>/dev/null || true)
    if [ -n "$owner" ] && ! kill -0 "$owner" 2>/dev/null; then
      rm -rf "$dir"
    fi
  done
  [ -n "$slot" ] || sleep 2
done
echo $$ > "$LOCK.$slot.d/pid"
trap 'rm -rf "$LOCK.$slot.d"' EXIT

# Мало памяти — подождать (другая сборка, игра, OBS).
while [ "$(free_kb)" -lt "$MIN_FREE_KB" ]; do
  sleep 5
done

# Потоки: все логические ядра, кроме HLSR_SERIAL_SPARE_CPUS (по умолчанию 2 — пользователю), делятся
# между местами; и не больше, чем позволяет свободная память (~1,2 ГБ на поток rustc, 3 ГБ запаса).
# nice -n 10 в Git Bash — класс приоритета Windows «ниже среднего»: rustc его наследует, и игры,
# браузер, OBS остаются отзывчивыми (решение владельца 2026-10-22).
if [ -z "$JOBS_SET" ]; then
  cpus=$(nproc 2>/dev/null || echo 8)
  per_slot=$(( (cpus - ${HLSR_SERIAL_SPARE_CPUS:-2}) / SLOTS ))
  mem_jobs=$(( ($(free_kb) / 1024 - 3000) / 1200 ))
  jobs=$(( per_slot < mem_jobs ? per_slot : mem_jobs ))
  [ "$jobs" -ge 2 ] || jobs=2
  export CARGO_BUILD_JOBS=$jobs
fi
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$MAIN/target}"
echo "serial.sh: место $slot, потоков $CARGO_BUILD_JOBS, CARGO_TARGET_DIR=$CARGO_TARGET_DIR" >&2
nice -n 10 "$@"
