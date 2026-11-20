#!/bin/bash
# Запустить команду в невидимом сеансе: KWin --virtual со своим Xwayland и своей
# шиной D-Bus. На экран и в трей сеанса пользователя ничего не попадает.
#
#   scripts/slint/headless.sh [команда и аргументы…]
#
# KWin и службы KDE в этом сеансе пишут настройки (kwinoutputconfig.json, kwinrc,
# kglobalshortcutsrc…) — поэтому XDG_CONFIG_HOME/STATE/CACHE/DATA_HOME временные,
# настройки fontconfig и пользовательские шрифты подкладываются для того же
# подбора шрифтов. Переменные:
#   HLSR_HEADLESS_SIZE=2560x1440 — размер виртуального монитора, физические px;
#   HLSR_HEADLESS_SCALE=1        — системный масштаб X11: Xft.dpi и
#                                  WINIT_X11_SCALE_FACTOR (XSETTINGS KWin даёт
#                                  winit 96 DPI, Xft.dpi он не читает).
# Код выхода — код команды.
set -euo pipefail

size=${HLSR_HEADLESS_SIZE:-2560x1440}
scale=${HLSR_HEADLESS_SCALE:-1}
dpi=$(awk -v s="$scale" 'BEGIN { printf "%d", 96 * s + 0.5 }')
root=$(mktemp -d "${TMPDIR:-/tmp}/hlsr-headless.XXXXXX")
trap 'rm -rf "$root" 2>/dev/null || true' EXIT
mkdir -p "$root/config" "$root/state" "$root/cache" "$root/data"
if [ -d "$HOME/.config/fontconfig" ]; then cp -r "$HOME/.config/fontconfig" "$root/config/"; fi
if [ -d "$HOME/.local/share/fonts" ]; then ln -s "$HOME/.local/share/fonts" "$root/data/fonts"; fi
# XTest проверок (python3) внутри сеанса — без запроса KWin на эмуляцию ввода.
printf '[Xwayland]\nXwaylandEisNoPromptApps=python3\n' > "$root/config/kwinrc"

# KWin запускает сеанс одной командой: обёртка ставит Xft.dpi и сохраняет код выхода.
{
    printf '#!/bin/bash\n'
    printf 'cd %q\n' "$PWD"
    printf 'printf "Xft.dpi: %%s\\n" %q | xrdb -merge\n' "$dpi"
    printf 'export WINIT_X11_SCALE_FACTOR=%q\n' "$scale"
    printf '%s\n' "$(printf '%q ' "$@")"
    printf 'echo $? > %q\n' "$root/status"
} > "$root/session.sh"
chmod +x "$root/session.sh"

# Без WAYLAND_DISPLAY и DISPLAY пользователя: службы, которые запустит шина D-Bus
# сеанса (порталы, kded), иначе подключились бы к его экрану. HLSR_HEADLESS_ROOT —
# метка: по ней после сеанса завершаются оставшиеся процессы.
env -u WAYLAND_DISPLAY -u DISPLAY HLSR_HEADLESS_ROOT="$root" \
    XDG_CONFIG_HOME="$root/config" XDG_STATE_HOME="$root/state" \
    XDG_CACHE_HOME="$root/cache" XDG_DATA_HOME="$root/data" \
    dbus-run-session -- kwin_wayland --virtual --xwayland \
        --socket "hlsr-headless-$$" --width "${size%x*}" --height "${size#*x}" \
        --no-lockscreen --no-global-shortcuts --no-kactivities \
        --exit-with-session "$root/session.sh" 2>"$root/kwin.log" || true

status=125
if [ -s "$root/status" ]; then
    status=$(cat "$root/status")
else
    echo "headless.sh: сеанс KWin не запустился" >&2
    tail -20 "$root/kwin.log" >&2
fi

# Службы шины сеанса (порталы, ksecretd и т. п.) переживают dbus-run-session.
session_pids() {
    local env pid
    for env in /proc/[0-9]*/environ; do
        if { tr '\0' '\n' < "$env"; } 2>/dev/null | grep -qxF "HLSR_HEADLESS_ROOT=$root"; then
            pid=${env#/proc/}
            echo "${pid%/environ}"
        fi
    done
}
for signal in TERM TERM TERM TERM KILL; do
    pids=$(session_pids)
    [ -z "$pids" ] && break
    # shellcheck disable=SC2086
    kill -s "$signal" $pids 2>/dev/null || true
    sleep 0.5
done
exit "$status"
