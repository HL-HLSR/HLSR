#!/usr/bin/env bash
# Windows: cargo check / clippy под x86_64-unknown-linux-gnu без компилятора C и sysroot Linux — проверка
# кода под cfg(target_os = "linux"). C и asm зависимостей (ring) не собираются: подделка
# fake-cc.cmd пишет пустые объекты и архивы; библиотеки Linux для build.rs крейтов -sys (fontconfig,
# x11, wayland — интерфейс, приложение, снимки; у GPUI ещё freetype2 для freetype-sys и alsa для
# alsa-sys звука cpal) «находит»
# подделка fake-pkg-config.cmd без флагов. libxkbcommon и libxkbcommon-x11 крейт xkbcommon (у gpui_linux)
# берёт через #[link], без build.rs: они нужны только при компоновке. libxcb x11rb открывает сам во время
# работы (dl-libxcb); крейт xcb в сборку не входит.
# check и clippy не компонуют. Тесты Linux так не запустить — только на Linux.
#   scripts/slint/serial.sh scripts/slint/check-linux.sh clippy -p hlsr-online --all-targets -- -D warnings
#   scripts/slint/serial.sh scripts/slint/check-linux.sh clippy -p hlsr-ui -p hlsr-app -p hlsr-snap --all-targets -- -D warnings
#   scripts/slint/serial.sh scripts/slint/check-linux.sh clippy -p hlsr-gpui --all-targets -- -D warnings
set -euo pipefail
dir="$(cd "$(dirname "$0")" && pwd -W)"
fake="$dir/fake-cc.cmd"
export CC_x86_64_unknown_linux_gnu="$fake" CXX_x86_64_unknown_linux_gnu="$fake" AR_x86_64_unknown_linux_gnu="$fake"
# Свой pkg-config для цели заодно разрешает его при сборке «на другую ОС» (PKG_CONFIG_ALLOW_CROSS не нужен).
export PKG_CONFIG_x86_64_unknown_linux_gnu="$dir/fake-pkg-config.cmd"
command="$1"
shift
exec cargo "$command" --target x86_64-unknown-linux-gnu "$@"
