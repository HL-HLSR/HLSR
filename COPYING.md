# Copying HLSR

The HLSR source code and documentation in this repository are licensed under
the GNU General Public License version 3 only (`GPL-3.0-only`). The full text
is in [`LICENSE`](LICENSE).

Код и документация HLSR в этом репозитории распространяются по GNU GPL
версии 3 (`GPL-3.0-only`). Ниже — что под эту лицензию не попадает.

## HLSR branding / Бренд HLSR

The HLSR name and logo are not licensed under the GPL. All rights reserved.
If you fork this repository and distribute a build, give it a different name
and remove these files or replace them with your own:

Название и логотип HLSR под GPL не распространяются. Изменённую сборку
назовите иначе, а эти файлы удалите или замените своими:

- `icons/hlsrc.png`, `icons/hlsr_mini.png`
- `build/icons/*`
- `crates/hlsr-ui/assets/logo.png`
- `crates/hlsr-ui/assets/launcher/badges/*.png` (except `bughunter.png`, see below),
  `crates/hlsr-ui/assets/launcher/subscriber-badge.png`

## Game material / Материалы игр

Half-Life, Half-Life 2, Steam and Source are trademarks of Valve Corporation.
HLSR is an unofficial community project, not affiliated with or endorsed by
Valve. The following files show material from Valve games (and, for one
sound, another game) for previews in the launcher. They belong to their
owners and are not licensed under the GPL:

HLSR — неофициальный проект сообщества, не связанный с Valve. Эти файлы
показывают материалы игр в превью лаунчера, принадлежат правообладателям и
под GPL не распространяются:

- `icons/bs.ico`, `icons/hl1.ico`, `icons/hl2.ico`, `icons/of.ico`
- `crates/hlsr-ui/assets/logos/*` (except `livesplit.svg`, see below)
- `crates/hlsr-ui/assets/glyphs/hl.svg`, `crates/hlsr-ui/assets/glyphs/lambda.svg`
- `crates/hlsr-ui/assets/launcher/`: `hero-hl2.jpg`, `hl1-bg.jpg`, `hl1-front.png`,
  `hl2-episodes-bg.jpg`, `hl2-front.webp`, `hl2-ghosting-bg.png`,
  `hl2-ghosting-front.webp` (game backgrounds and covers)
- `crates/hlsr-ui/assets/launcher/custom-packs/*`
- `crates/hlsr-ui/assets/launcher/gta-ui.wav`
- `crates/hlsr-ui/assets/hl1-preview/*`
- `crates/hlsr-ui/assets/ghosting-preview/*`

Other files that are not HLSR's own and are not licensed under the GPL:

- `crates/hlsr-ui/assets/logos/livesplit.svg` — the LiveSplit logo, belongs to
  the LiveSplit project;
- `crates/hlsr-ui/assets/launcher/badges/bughunter.png` — the Discord Bug
  Hunter badge, and `crates/hlsr-ui/assets/glyphs/discord.svg` — the Discord
  logo; both belong to Discord Inc.;
- `crates/hlsr-ui/assets/launcher/pony.webp` — an easter egg image, belongs to
  its author;
- `crates/hlsr-ui/assets/launcher/team/*` — avatars of the contributors, belong
  to them.

## Earlier versions / Ранние версии

Earlier HLSR versions were released under the MIT License. Code from those
versions that remains in this repository (for example the frozen 0.4.7
modules in `tools/compat-047`) is now distributed under the GPL as a whole,
and its original notice is kept here as the MIT License requires:

Ранние версии HLSR выходили по лицензии MIT. Код из них, оставшийся в
репозитории, теперь распространяется в составе проекта по GPL, а исходное
уведомление MIT сохранено ниже.

```
MIT License

Copyright (c) 2020-2026 HLSR contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Third-party components / Сторонние компоненты

Fonts, 7-Zip, LiveSplit components, umu-launcher, GPUI and the Rust crates
keep their own licenses. See [`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md)
and `build/THIRD-PARTY-NOTICES.txt`.

Game builds that the launcher downloads are not part of this repository and
are not covered by this license.
