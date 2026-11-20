"""Картинки эмодзи реакций ленты (ПП5, Ф4; решение 17 спецификации).

0.4.7 рисует REACTIONS (shared/news-contract.js: 👍 ❤️ 🔥 😮 🎉) цветным
системным шрифтом: на Linux (эталонные снимки) — Noto Color Emoji, на Windows —
Segoe UI Emoji (несвободный). Лаунчер вшивает 5 картинок — растры глифов Noto
Color Emoji как есть: PNG из таблицы CBDT байт в байт (единственный strike
109 ppem, 136×128, bearingX 0, bearingY 101, advance 136 — их же ждёт
crates/hlsr-ui/src/emoji.rs). Шрифт — SIL OFL 1.1; те же рисунки в репозитории
googlefonts/noto-emoji — Apache 2.0.

    python tools/reference/lib/emoji-images.py <NotoColorEmoji.ttf> crates/hlsr-ui/assets/emoji

Источник: Noto Color Emoji 2.038 (noto-emoji 2022-09-06, 4b1c5bf), файл с
sha256 2eeac855a08803c6d209f8eb74ed5f798af46e128bc93dd3913e04de57523a7c.
Шрифт другой версии или с другой раскладкой strike — отказ. Повторный запуск
даёт те же байты.
"""

import hashlib
import os
import sys

from fontTools.ttLib import TTFont

# Вид реакции → кодовая точка глифа (порядок REACTIONS 0.4.7; U+FE0F у ❤️ —
# селектор варианта без своего глифа).
REACTIONS = [
    ("like", 0x1F44D),
    ("love", 0x2764),
    ("fire", 0x1F525),
    ("wow", 0x1F62E),
    ("celebrate", 0x1F389),
]
VERSION = "Version 2.038;"
STRIKE = {"ppem": 109, "width": 136, "height": 128, "bearing_x": 0, "bearing_y": 101, "advance": 136}


def main(source: str, target: str) -> None:
    with open(source, "rb") as file:
        digest = hashlib.sha256(file.read()).hexdigest()
    font = TTFont(source)
    version = font["name"].getDebugName(5) or ""
    if not version.startswith(VERSION):
        sys.exit(f"{source}: нужен Noto Color Emoji 2.038, а это «{version}»")
    strikes = font["CBLC"].strikes
    if len(strikes) != 1 or strikes[0].bitmapSizeTable.ppemY != STRIKE["ppem"]:
        sys.exit(f"{source}: ждали один strike {STRIKE['ppem']} ppem")
    data = font["CBDT"].strikeData[0]
    cmap = font.getBestCmap()
    os.makedirs(target, exist_ok=True)
    for key, codepoint in REACTIONS:
        glyph = data[cmap[codepoint]]
        glyph.ensureDecompiled()
        metrics = glyph.metrics
        got = {
            "ppem": STRIKE["ppem"],
            "width": metrics.width,
            "height": metrics.height,
            "bearing_x": metrics.BearingX,
            "bearing_y": metrics.BearingY,
            "advance": metrics.Advance,
        }
        if got != STRIKE:
            sys.exit(f"U+{codepoint:04X}: метрики {got}, ждали {STRIKE}")
        if not glyph.imageData.startswith(b"\x89PNG\r\n\x1a\n"):
            sys.exit(f"U+{codepoint:04X}: в CBDT не PNG")
        with open(os.path.join(target, f"{key}.png"), "wb") as file:
            file.write(glyph.imageData)
    print(f"{version} sha256 {digest}: {len(REACTIONS)} картинок в {target}")


if __name__ == "__main__":
    # Консоль Windows (cp1251) не знает эмодзи и стрелок из справки.
    for stream in (sys.stdout, sys.stderr):
        stream.reconfigure(errors="replace")
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
