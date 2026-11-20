"""Запасной шрифт символов лаунчера (ПП5, Ф4; решение 17 спецификации).

В Rubik нет стрелок, «⌘», «✓» и «●»: Chromium 0.4.7 берёт их из системных
шрифтов, у Slint цепочка своя. Вшивается подмножество Inter (SIL OFL 1.1, тот же
файл, что Slint кладёт в i-slint-common как запасной шрифт) — начертание 400,
только эти символы, семейство переименовано в «HLSR Symbols» (у Inter
зарезервированное имя, OFL-1.1-RFN).

    python tools/reference/lib/symbols-font.py <Inter-VariableFont.ttf> crates/hlsr-ui/fonts/HLSRSymbols-Regular.ttf

Источник: ~/.cargo/registry/src/*/i-slint-common-1.18.1/sharedfontique/Inter-VariableFont.ttf.
Повторный запуск даёт те же байты (время в head не пересчитывается).
"""

import sys

from fontTools import subset
from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

# ↑ ↓ ← → ↵ ⌘ ✓ ● и пробел (ширина пробела нужна раскладке); ↵ — клавиша Num Enter схемы клавиатуры.
CODEPOINTS = [0x20, 0x2190, 0x2191, 0x2192, 0x2193, 0x21B5, 0x2318, 0x2713, 0x25CF]
FAMILY = "HLSR Symbols"
POSTSCRIPT = "HLSRSymbols-Regular"


def main(source: str, target: str) -> None:
    font = TTFont(source, recalcTimestamp=False)
    font = instancer.instantiateVariableFont(font, {"wght": 400, "opsz": None} if "opsz" in [a.axisTag for a in font["fvar"].axes] else {"wght": 400})
    options = subset.Options()
    options.layout_features = ["*"]
    options.name_IDs = ["*"]
    options.name_languages = ["*"]
    options.notdef_outline = True
    options.recalc_timestamp = False
    options.drop_tables += ["STAT", "DSIG"]
    subsetter = subset.Subsetter(options)
    subsetter.populate(unicodes=CODEPOINTS)
    subsetter.subset(font)

    names = font["name"]
    copyright_ = names.getDebugName(0)
    version = names.getDebugName(5)
    license_ = names.getDebugName(13)
    license_url = names.getDebugName(14)
    names.names = []
    for name_id, value in (
        (0, copyright_ + ". Subset and renamed for HLSR (Reserved Font Name \"Inter\" not used)."),
        (1, FAMILY),
        (2, "Regular"),
        (3, f"{version};{POSTSCRIPT}"),
        (4, f"{FAMILY} Regular"),
        (5, version),
        (6, POSTSCRIPT),
        (13, license_ or "This Font Software is licensed under the SIL Open Font License, Version 1.1."),
        (14, license_url or "https://openfontlicense.org"),
    ):
        names.setName(value, name_id, 3, 1, 0x409)
    font["OS/2"].usWeightClass = 400
    font.save(target)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
