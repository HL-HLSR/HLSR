#!/usr/bin/env python3
"""Список крейтов Rust в лаунчере для build/THIRD-PARTY-NOTICES.txt — без сети.

Крейты, которые собираются в hlsr / HLSR.exe: обычные зависимости hlsr-app с вложенными, без dev-
и build-зависимостей, без процедурных макросов (они работают при сборке и в exe не входят) и без
своих крейтов workspace (у них LICENSE проекта; изменённые копии чужих крейтов в vendor/ — в списке,
с пометкой); признаки — как у `cargo build -p hlsr-app`
(выпуск без dev-hooks). Для каждой цели выпуска — вывод `cargo tree` по Cargo.lock: имя, версия,
лицензия из Cargo.toml крейта (SPDX). Блок между метками в build/THIRD-PARTY-NOTICES.txt
заменяется; повторный запуск даёт те же байты.

    for t in x86_64-pc-windows-msvc x86_64-unknown-linux-gnu; do
      scripts/slint/serial.sh cargo tree --offline --locked -p hlsr-app -e normal,no-proc-macro \\
        --target $t --prefix none -f '{p}|{l}' > target/crates-$t.txt
    done
    python scripts/slint/crate-notices.py target/crates-x86_64-pc-windows-msvc.txt \\
        target/crates-x86_64-unknown-linux-gnu.txt

--check — только сверить блок (код 1, если устарел).
"""

import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
NOTICES = os.path.join(ROOT, "build", "THIRD-PARTY-NOTICES.txt")
BEGIN = "=== BEGIN GENERATED RUST CRATE INVENTORY (scripts/slint/crate-notices.py) ==="
END = "=== END GENERATED RUST CRATE INVENTORY ==="


def crates_of(path):
    """{(имя, версия): лицензия} из вывода `cargo tree -f '{p}|{l}' --prefix none`."""
    result = {}
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.rstrip("\r\n")
            if not line:
                continue
            if line.endswith(" (*)"):  # поддерево уже выведено выше
                line = line[: -len(" (*)")]
            package, _, license_ = line.partition("|")
            name, _, version = package.partition(" v")
            license_ = license_ or "UNKNOWN"
            if " (" in version:
                version, _, where = version.partition(" (")
                # Изменённая копия чужого крейта в vendor/ (её HLSR-PATCH.md) — в списке остаётся.
                if "vendor" not in where.replace("\\", "/").split("/"):
                    continue  # свой крейт workspace: путь в скобках
                license_ += f" (modified copy: vendor/{name})"
            result[(name, version)] = license_
    return result


def version_key(version):
    parts = []
    for part in version.split("+")[0].split("-")[0].split("."):
        parts.append(int(part) if part.isdigit() else 0)
    return parts


def block(windows, linux):
    names = sorted(set(windows) | set(linux), key=lambda key: (key[0], version_key(key[1])))
    lines = [
        BEGIN,
        "",
        "Rust crates compiled into the launcher executable (HLSR.exe on Windows, hlsr",
        "on Linux), transitive dependencies included, as resolved by Cargo.lock. The",
        "license is the SPDX expression declared by each crate; where it offers a",
        "choice (OR), HLSR uses the crate under one of the listed licenses. The full",
        "license texts are in the source package of each crate:",
        "https://crates.io/crates/<name>/<version>.",
        "W - Windows build, L - Linux build.",
        "",
    ]
    width = max(len(f"{name}@{version}") for name, version in names) + 2
    for key in names:
        name, version = key
        license_ = windows.get(key) or linux.get(key)
        where = ("W" if key in windows else " ") + " " + ("L" if key in linux else " ")
        lines.append(f"{name}@{version}".ljust(width) + where + "  " + license_)
    lines += ["", f"Crates: {len(names)}.", "", END]
    return "\n".join(lines), len(names)


def main(argv):
    check = "--check" in argv
    paths = [arg for arg in argv if not arg.startswith("--")]
    if len(paths) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    text, count = block(crates_of(paths[0]), crates_of(paths[1]))
    with open(NOTICES, encoding="utf-8", newline="") as f:
        current = f.read()
    start = current.find(BEGIN)
    stop = current.find(END)
    if start < 0 or stop < start:
        print(f"нет меток блока в {NOTICES}", file=sys.stderr)
        return 1
    updated = current[:start] + text + current[stop + len(END) :]
    if check:
        if updated != current:
            print("список крейтов в THIRD-PARTY-NOTICES.txt устарел: запустите без --check", file=sys.stderr)
            return 1
        print(f"список крейтов совпадает с Cargo.lock: {count}")
        return 0
    with open(NOTICES, "w", encoding="utf-8", newline="") as f:
        f.write(updated)
    print(f"записан {os.path.relpath(NOTICES, ROOT)}: крейтов {count}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
