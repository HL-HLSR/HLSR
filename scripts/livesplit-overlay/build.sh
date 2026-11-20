#!/bin/sh
# Сборка оверлея LiveSplit, без CRT, на Linux: clang + lld и заголовки и
# библиотеки импорта Wine. Результат кладётся в resources/livesplit-overlay/
# и коммитится: на Windows и в CI его не собирают.
#
#   proxy-d3d9.dll       прокси d3d9.dll (HL2 2707/4260, Ghosting), 32 бита
#   proxy-opengl32.dll   прокси opengl32.dll (HL1 WON), 32 бита
#   hlsr-share64.dll    снимок окна изнутри LiveSplit под Wine, 64 бита
#   hlsr-overlay64.exe   внедряет hlsr-share64.dll в LiveSplit под Wine, 64 бита
#
# Лаунчер кладёт прокси в папку игры под именем настоящей DLL (см. proxy.h);
# последние два файла нужны только на Linux.
#
#   sh scripts/livesplit-overlay/build.sh
#
# Сборка воспроизводимая: без отметок времени, повторный запуск даёт те же байты.
#
# ВРЕМЕННО: proxy-d3d9.dll и proxy-opengl32.dll в resources/livesplit-overlay
# собраны MSVC на Windows (build-msvc.cmd, Build Tools 2022 x86, без CRT, те же
# .def) — исправления таблиц методов и выгрузки прокси проверялись там.
# Пересобрать этим скриптом на Linux и закоммитить результат; остальные файлы
# собраны им.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
out="$here/../../resources/livesplit-overlay"
wine_include=${WINE_INCLUDE:-/usr/include/wine}
wine_lib=${WINE_LIB:-/usr/lib/wine/i386-windows}
wine_lib64=${WINE_LIB64:-/usr/lib/wine/x86_64-windows}
mkdir -p "$out"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

common="-O2 -fno-math-errno -ffreestanding \
  -fno-stack-protector -fno-builtin -fno-ident -nostdlibinc -isystem $wine_include/msvcrt \
  -isystem $wine_include/windows -D_WIN32_WINNT=0x0601 -DWINVER=0x0601 -Wall -Wextra -Wno-unused-parameter"
cflags="--target=i686-w64-windows-gnu -msse2 -mfpmath=sse $common"
cflags64="--target=x86_64-w64-windows-gnu -mno-stack-arg-probe $common"
link="-fuse-ld=lld -nostdlib -Wl,--no-insert-timestamp -Wl,--build-id=none -Wl,--dynamicbase -Wl,--nxcompat"
ldflags="$link -L$wine_lib"
ldflags64="$link -L$wine_lib64"

# opengl32.def: переходник на каждую функцию из списка и перехват wglSwapBuffers.
{
  echo "LIBRARY opengl32.dll"
  echo "EXPORTS"
  sed -n 's/^X(\(.*\))$/  \1 = thunk_\1/p' "$here/proxy-gl-exports.inc"
  echo "  wglSwapBuffers = hook_wglSwapBuffers@4"
} > "$work/opengl32.def"

clang $cflags -shared "$here/proxy-d3d9.c" "$here/proxy-d3d9.def" -o "$out/proxy-d3d9.dll" $ldflags \
  -Wl,--entry=DllMain@12 -Wl,--subsystem,windows -lkernel32 -luser32 -lgdi32 -lntdll
clang $cflags -shared "$here/proxy-gl.c" "$work/opengl32.def" -o "$out/proxy-opengl32.dll" $ldflags \
  -Wl,--entry=DllMain@12 -Wl,--subsystem,windows -lkernel32 -luser32 -lgdi32 -lntdll
clang $cflags64 -shared "$here/share.c" -o "$out/hlsr-share64.dll" $ldflags64 \
  -Wl,--entry=DllMain -Wl,--subsystem,windows -lkernel32 -luser32 -lgdi32 -lntdll
clang $cflags64 "$here/inject.c" -o "$out/hlsr-overlay64.exe" $ldflags64 \
  -Wl,--entry=start -Wl,--subsystem,console -lkernel32 -luser32 -lgdi32 -lshell32 -lntdll
sha256sum "$out/proxy-d3d9.dll" "$out/proxy-opengl32.dll" "$out/hlsr-share64.dll" "$out/hlsr-overlay64.exe"
