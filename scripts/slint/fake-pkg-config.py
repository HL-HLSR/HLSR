"""Подделка pkg-config для check-linux.sh: любая библиотека Linux (fontconfig, x11, wayland…) «есть», без флагов.

cargo check и clippy не компонуют, поэтому build.rs крейтов -sys (yeslogic-fontconfig-sys, wayland-sys,
tiny-xlib, x11-dl; у GPUI — freetype-sys, иначе он собрал бы свою freetype) хватает успешного ответа:
пустые --libs/--cflags, условная версия и libdir.
"""

import sys

args = sys.argv[1:]
variable = next((a.split("=", 1)[1] for a in args if a.startswith("--variable=")), None)
if variable == "libdir":
    print("/usr/lib/x86_64-linux-gnu")
elif variable is not None:
    print()
elif "--modversion" in args:
    print("99.0")
else:
    print()
