"""Подделка компилятора C и ar для check-linux.sh: вместо объектов и архивов — пустые файлы.

cargo check и clippy не компонуют, поэтому C и asm зависимостей (ring) им не нужны.
"""

import re
import sys

args = sys.argv[1:]
if "-o" in args:
    # cc … -c файл.c -o объект.o
    open(args[args.index("-o") + 1], "wb").close()
elif len(args) >= 2 and re.fullmatch(r"[a-zA-Z]+", args[0]) and args[1].endswith(".a"):
    # ar cq архив.a объекты… / ar s архив.a
    open(args[1], "ab").close()
