# Copying HLSR 0.4.x

The HLSR source code and documentation in this branch are licensed under the
GNU General Public License version 3 only (`GPL-3.0-only`). The full text is
in [`LICENSE`](LICENSE).

Код и документация HLSR в этой ветке распространяются по GNU GPL версии 3
(`GPL-3.0-only`). Ниже — что под эту лицензию не попадает.

## HLSR branding / Бренд HLSR

The HLSR name and logo are not licensed under the GPL. All rights reserved.
If you fork this code and distribute a build, give it a different name and
remove these files or replace them with your own:

Название и логотип HLSR под GPL не распространяются. Изменённую сборку
назовите иначе, а эти файлы удалите или замените своими:

- `icons/hlsrc.png`, `icons/hlsr_mini.png`
- `build/icons/*`
- `src/renderer/assets/logo.png`
- `src/renderer/assets/launcher/badges/*.png` (except `bughunter.png`, see below),
  `src/renderer/assets/launcher/subscriber-badge.png`

## Game material / Материалы игр

Half-Life, Half-Life 2, Steam and Source are trademarks of Valve Corporation.
HLSR is an unofficial community project, not affiliated with or endorsed by
Valve. The following files show material from Valve games (and, for one
sound, another game) in the launcher. They belong to their owners and are not
licensed under the GPL:

HLSR — неофициальный проект сообщества, не связанный с Valve. Эти файлы
показывают материалы игр в лаунчере, принадлежат правообладателям и под GPL
не распространяются:

- `icons/bs.ico`, `icons/hl1.ico`, `icons/hl2.ico`, `icons/of.ico`
- `src/renderer/assets/`: `bs.jpg`, `hl.jpg`, `of.jpg`, `lancer.jpg`,
  `lambda.svg`, `screenshots/*`
- `src/renderer/assets/launcher/`: `hero-hl2.jpg`, `hl1-bg.jpg`,
  `hl1-front.png`, `hl2-episodes-bg.jpg`, `hl2-front.webp`,
  `hl2-ghosting-bg.png`, `hl2-ghosting-front.webp`, `hl2-ghosting-poster.jpg`,
  `hl2-logo.webp`, `ghosting-personalization-preview.webp`, `gta-ui.mp3`
- `src/renderer/assets/launcher/logos/*` (except `livesplit.svg`, see below)
- `src/renderer/assets/launcher/custom-packs/*`
- `src/renderer/assets/launcher/hl1-preview/*`
- `src/renderer/assets/launcher/ghosting-preview/*`

Other files that are not HLSR's own and are not licensed under the GPL:

- `src/renderer/assets/launcher/logos/livesplit.svg` — the LiveSplit logo,
  belongs to the LiveSplit project;
- `src/renderer/assets/launcher/badges/bughunter.png` — the Discord Bug
  Hunter badge, belongs to Discord Inc.;
- `icons/badges/*` — Twitch and YouTube chat badges, belong to Twitch
  Interactive, Inc. and Google LLC;
- `src/renderer/assets/launcher/pony.webp` — an easter egg image, belongs to
  its author;
- `src/renderer/assets/launcher/team/*` — avatars of the contributors, belong
  to them.

## Earlier versions / Ранние версии

Earlier HLSR versions were released under the MIT License. Their code is now
distributed under the GPL as a whole, and the original notice is kept here as
the MIT License requires:

Ранние версии HLSR выходили по лицензии MIT. Их код теперь распространяется
в составе проекта по GPL, а исходное уведомление MIT сохранено ниже.

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

Fonts, 7-Zip, LiveSplit components, umu-launcher, Electron, Vue and the npm
packages keep their own licenses. See
[`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md) and
`build/THIRD-PARTY-NOTICES.txt`.

Game builds that the launcher downloads are not part of this repository and
are not covered by this license.
