#!/bin/sh
# Сборка hlsr-presence-relay.exe (мост статуса игры для Discord под Proton)
# без CRT, на Linux: clang + lld и заголовки и библиотеки импорта Wine.
# Результат кладётся в resources/presence-relay/ и коммитится: на Windows
# помощник не нужен, а в CI его не собирают.
#
#   sh scripts/presence-relay/build.sh
#
# Подсистема windows, а не console: у консольной программы в префиксе игры
# появляется лишнее окно conhost.
#
# Сборка воспроизводимая: без отметок времени, повторный запуск даёт те же байты.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
out="$here/../../resources/presence-relay"
wine_include=${WINE_INCLUDE:-/usr/include/wine}
wine_lib=${WINE_LIB:-/usr/lib/wine/i386-windows}
mkdir -p "$out"

clang --target=i686-w64-windows-gnu -O2 -ffreestanding -fno-stack-protector -fno-builtin -fno-ident \
  -nostdlibinc -isystem "$wine_include/msvcrt" -isystem "$wine_include/windows" \
  -D_WIN32_WINNT=0x0601 -DWINVER=0x0601 -Wall -Wextra -Wno-unused-parameter \
  "$here/relay.c" -o "$out/hlsr-presence-relay.exe" \
  -fuse-ld=lld -nostdlib -Wl,--no-insert-timestamp -Wl,--build-id=none -Wl,--dynamicbase -Wl,--nxcompat \
  -L"$wine_lib" -Wl,--entry=start@0 -Wl,--subsystem,windows -lkernel32 -luser32 -lshell32 -lws2_32 -lntdll
sha256sum "$out/hlsr-presence-relay.exe"
