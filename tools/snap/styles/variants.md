# Варианты одноимённых классов 0.4.7

Сгенерировано `node tools/reference/css-inventory.js styles` (`tools/reference/lib/style-map.js`) из `<style>` всех `.vue`, `.css` и `.scss`
рендерера 0.4.7; вложенность SCSS раскрыта (`lib/css-rules.js`). Блок — первый класс последней составной части селектора без
`__элемента` и `--модификатора`. Копия — блок, у которого хотя бы один селектор объявлен в двух и более файлах (карта §0.8, §2
«Варианты»): это scoped-копии, а не общие стили, и общему компоненту Slint нужен параметр на каждое различие. Для селектора из
нескольких файлов — таблица свойств с разными значениями (— значит, свойство не задано) и строка «одинаково»; селекторы одного
файла — списком с объявлениями. Значения — как в исходнике (`var()`, `calc()`, `color-mix()`), `@include` — миксины SCSS;
переменные `--x` опущены (они в `crates/hlsr-ui/themes.json`). Блоки, которые второй файл лишь дополняет своими селекторами
(темы в `launcher.css`, вложенные правила), — отдельным списком без разбора.

| Копия | Файлов | Селекторов | Общих | С различиями |
|---|---|---|---|---|
| `launcher-root` | 2 | 14 | 1 | 1 |
| `settings-btn` | 4 | 29 | 14 | 6 |
| `settings-group` | 2 | 2 | 2 | 2 |
| `settings-link` | 2 | 3 | 3 | 2 |
| `settings-modal` | 8 | 28 | 8 | 6 |
| `settings-row` | 3 | 9 | 6 | 0 |
| `settings-select` | 3 | 5 | 5 | 3 |
| `settings-switch` | 4 | 17 | 8 | 6 |

Блоки, дополняемые из другого файла (общих селекторов нет):

- `account-menu`: `assets/css/launcher.css`, `components/Launcher/AccountMenu.vue`
- `avatar-frame`: `assets/css/launcher.css`, `components/Launcher/AvatarFrame.vue`
- `build-panel`: `assets/css/launcher.css`, `components/Launcher/BuildSelector.vue`
- `confirm`: `assets/css/launcher.css`, `components/Launcher/ConfirmModal.vue`
- `diag`: `assets/css/launcher.css`, `components/Launcher/SettingsModal.vue`
- `download-bar`: `assets/css/launcher.css`, `components/Launcher/DownloadBar.vue`
- `friends`: `assets/css/launcher.css`, `components/Launcher/FriendsPanel.vue`
- `game-rail`: `components/Launcher/GameSidebar.vue`, `components/Launcher/LauncherShell.vue`
- `gp`: `assets/css/launcher.css`, `components/Launcher/GhostingSettingsPreview.vue`
- `gx-btn`: `components/Launcher/GameCfgFiles.vue`, `components/Launcher/LiveSplitOverlayEditor.vue`, `components/Launcher/settings-fields.scss`
- `gx-fields`: `components/Launcher/GameConfigEditor.vue`, `components/Launcher/settings-fields.scss`
- `gx-input`: `components/Launcher/LiveSplitOverlayEditor.vue`, `components/Launcher/settings-fields.scss`
- `gx-row`: `components/Launcher/GameConfigEditor.vue`, `components/Launcher/settings-fields.scss`
- `library-setup`: `assets/css/launcher.css`, `components/Launcher/LibrarySetupModal.vue`
- `news-viewer`: `assets/css/launcher.css`, `components/Launcher/NewsImageViewer.vue`
- `profile`: `assets/css/launcher.css`, `components/Launcher/ProfileModal.vue`
- `toasts`: `assets/css/launcher.css`, `components/Launcher/ToastContainer.vue`
- `ui-dropdown`: `components/Launcher/BuildSelector.vue`, `components/Launcher/UiDropdown.vue`, `components/Launcher/settings-fields.scss`

## launcher-root — 2 файла

`assets/css/launcher.css:10`, `components/Launcher/LauncherShell.vue:2009`

### `.launcher-root`

| свойство | `launcher.css` | `LauncherShell.vue` |
|---|---|---|
| `background-color` | — | `var(--l-bg)` |
| `background-image` | — | `var(--l-app-scrim), var(--l-app-image, none)` |
| `background-position` | — | `center, center` |
| `background-repeat` | — | `no-repeat, no-repeat` |
| `background-size` | — | `cover, cover` |
| `font-family` | — | `Rubik, sans-serif` |
| `inset` | — | `0` |
| `overflow` | — | `hidden` |
| `position` | — | `absolute` |
| `z-index` | — | `10` |

Одинаковых свойств нет.

### Только в одном файле

- `launcher.css` — `.launcher-root :focus-visible`: outline: 2px solid var(--accent-focus); outline-offset: 2px
- `launcher.css` — `.launcher-root.no-anim *`: transition-duration: 0.01ms; animation-duration: 0.01ms
- `launcher.css` — `.launcher-root.no-anim *::after`: transition-duration: 0.01ms; animation-duration: 0.01ms
- `launcher.css` — `.launcher-root.no-anim *::before`: transition-duration: 0.01ms; animation-duration: 0.01ms
- `launcher.css` — `.launcher-root[data-theme="liquid-glass"]`: --lg-filter: blur(14px) url(#lg-refract) saturate(170%) brightness(1.06); --lg-filter-panel: blur(8px) url(#lg-refract-strong) saturate(160%); --lg-filter-soft: blur(6px) url(#lg-refract-strong) saturate(140%); --lg-tint: rgba(255, 255, 255, 0.07); --lg-rim: inset 2px 2px 1px 0 rgba(255, 255, 255, 0.28), inset -1px -1px 1px 1px rgba(255, 255, 255, 0.14); --lg-shadow: 0 6px 6px rgba(0, 0, 0, 0.2), 0 0 20px rgba(0, 0, 0, 0.1), 0 24px 60px rgba(0, 0, 0, 0.28); --lg-sheen: linear-gradient(150deg, rgba(255, 255, 255, 0.14) 0%, rgba(255, 255, 255, 0.04) 30%, rgba(255, 255, 255, 0) 55%, rgba(255, 255, 255, 0.06) 100%)
- `launcher.css` — `@media (max-height: 663px) › .launcher-root`: --l-rail-item: 48px; --l-rail-circle: 40px; --l-hero-top: 184px; --l-hero-gap: 28px; --l-stats-gap: 24px
- `launcher.css` — `@media (max-width: 1295px) › .launcher-root`: --l-header-gap: 24px
- `launcher.css` — `@media (min-height: 708px) › .launcher-root`: --l-rail-gap: 8px; --l-rail-tool-gap: 8px; --l-rail-bottom: 16px; --l-hero-pad: 56px; --l-hero-top: 208px; --l-bottombar-gap: 40px
- `launcher.css` — `@media (min-height: 780px) › .launcher-root`: --l-topbar-h: 88px; --l-rail-w: 88px; --l-rail-tool: 48px; --l-rail-bottom: 20px; --l-hero-pad: 64px; --l-hero-top: 232px; --l-hero-gap: 36px; --l-stats-gap: 32px; --l-stats-value: 20px; --l-bottombar-h: 104px; --l-bottombar-lead: 40px; --l-play-w: 272px; --l-play-h: 64px; --l-play-font: 21px; --l-play-icon: 18px
- `launcher.css` — `@media (min-height: 860px) › .launcher-root`: --l-topbar-h: 96px; --l-header-control: 48px; --l-brand-mark: 48px; --l-rail-w: 96px; --l-rail-item: 64px; --l-rail-circle: 56px; --l-rail-logo: 40px; --l-rail-gap: 12px; --l-rail-bottom: 24px; --l-hero-pad: 72px; --l-hero-top: 256px; --l-hero-gap: 40px; --l-hero-title: 68px; --l-hero-title-lh: 72px; --l-hero-subtitle-lh: 32px; --l-stats-gap: 36px; --l-stats-icon: 56px; --l-stats-value: 22px; --l-stats-value-lh: 28px; --l-stats-texts-gap: 8px; --l-bottombar-h: 112px; --l-bottombar-gap: 48px; --l-play-w: 304px; --l-play-h: 72px; --l-play-font: 23px; --l-play-icon: 20px; --l-build-h: 64px
- `launcher.css` — `@media (prefers-reduced-motion: reduce) › .launcher-root *`: transition-duration: 0.01ms; animation-duration: 0.01ms
- `launcher.css` — `@media (prefers-reduced-motion: reduce) › .launcher-root *::after`: transition-duration: 0.01ms; animation-duration: 0.01ms
- `launcher.css` — `@media (prefers-reduced-motion: reduce) › .launcher-root *::before`: transition-duration: 0.01ms; animation-duration: 0.01ms

## settings-btn — 4 файла

`components/Launcher/GameSettingsModal.vue:1758`, `components/Launcher/RunnerSettings.vue:947`, `components/Launcher/SettingsModal.vue:1392`, `components/Launcher/UnavailableModal.vue:375`

### `.settings-btn`

| свойство | `GameSettingsModal.vue` | `RunnerSettings.vue` | `SettingsModal.vue` | `UnavailableModal.vue` |
|---|---|---|---|---|
| `border` | `1px solid var(--l-border-strong)` | `none` | `none` | `1px solid var(--l-border-strong)` |
| `box-shadow` | — | `inset 0 0 0 1px var(--l-border-strong)` | `inset 0 0 0 1px var(--l-border-strong)` | — |
| `display` | `inline-flex` | `inline-flex` | `inline-flex` | `flex` |
| `flex` | — | `none` | — | — |
| `font-size` | `13.5px` | `13px` | `13.5px` | `13px` |
| `font-weight` | `600` | `600` | `600` | `500` |
| `gap` | `8px` | `8px` | `8px` | `9px` |
| `justify-content` | `center` | `center` | `center` | — |
| `line-height` | `24px` | `24px` | `24px` | — |
| `min-height` | `48px` | `40px` | `48px` | — |
| `padding` | `11px 19px` | `8px 16px` | `12px 20px` | `9px 16px` |
| `transition` | `background var(--l-fast), border-color var(--l-fast), box-shadow var(--l-fast), color var(--l-fast), transform var(--l-fast)` | `background var(--l-fast), transform var(--l-fast)` | `background var(--l-fast), border-color var(--l-fast), transform var(--l-fast)` | `background var(--l-fast)` |
| `white-space` | — | `nowrap` | — | — |

Одинаково: `align-items: center`, `background: rgba(255, 255, 255, 0.05)`, `border-radius: var(--l-radius-sm)`, `color: var(--l-text)`, `cursor: pointer`, `font-family: inherit`.

### `.settings-btn i`

| свойство | `RunnerSettings.vue` | `SettingsModal.vue` | `UnavailableModal.vue` |
|---|---|---|---|
| `color` | — | — | `var(--l-text-dim)` |
| `font-size` | `12px` | `13px` | — |
| `opacity` | `0.9` | `0.9` | — |

Только в: `RunnerSettings.vue`, `SettingsModal.vue`, `UnavailableModal.vue`.

Одинаковых свойств нет.

### `.settings-btn.ghost`

| свойство | `GameSettingsModal.vue` | `RunnerSettings.vue` | `SettingsModal.vue` |
|---|---|---|---|
| `border-color` | — | — | `transparent` |
| `box-shadow` | — | `none` | — |

Только в: `GameSettingsModal.vue`, `RunnerSettings.vue`, `SettingsModal.vue`.

Одинаково: `background: transparent`, `color: var(--l-text-dim)`.

### `.settings-btn.ghost:disabled:hover`

Только в: `RunnerSettings.vue`, `SettingsModal.vue`.

Одинаково: `background: transparent`, `color: var(--l-text-dim)`.

### `.settings-btn.ghost:hover`

Только в: `RunnerSettings.vue`, `SettingsModal.vue`.

Одинаково: `background: rgba(255, 255, 255, 0.06)`, `color: var(--l-text)`.

### `.settings-btn.primary`

| свойство | `GameSettingsModal.vue` | `RunnerSettings.vue` | `SettingsModal.vue` |
|---|---|---|---|
| `border-color` | `transparent` | — | `transparent` |
| `box-shadow` | — | `none` | — |

Только в: `GameSettingsModal.vue`, `RunnerSettings.vue`, `SettingsModal.vue`.

Одинаково: `background: var(--accent)`, `color: var(--accent-contrast-text)`.

### `.settings-btn.primary:active`

Только в: `RunnerSettings.vue`, `SettingsModal.vue`.

Одинаково: `background: var(--accent-pressed)`.

### `.settings-btn.primary:disabled:hover`

Только в: `RunnerSettings.vue`, `SettingsModal.vue`.

Одинаково: `background: var(--accent)`.

### `.settings-btn.primary:hover`

Только в: `RunnerSettings.vue`, `SettingsModal.vue`.

Одинаково: `background: var(--accent-hover)`.

### `.settings-btn:active`

Только в: `RunnerSettings.vue`, `SettingsModal.vue`.

Одинаково: `transform: scale(0.97)`.

### `.settings-btn:disabled`

| свойство | `GameSettingsModal.vue` | `RunnerSettings.vue` | `SettingsModal.vue` |
|---|---|---|---|
| `cursor` | `not-allowed` | `default` | `default` |
| `transform` | — | `none` | `none` |

Только в: `GameSettingsModal.vue`, `RunnerSettings.vue`, `SettingsModal.vue`.

Одинаково: `opacity: 0.45`.

### `.settings-btn:disabled:hover`

Только в: `RunnerSettings.vue`, `SettingsModal.vue`.

Одинаково: `background: rgba(255, 255, 255, 0.05)`.

### `.settings-btn:focus-visible`

| свойство | `GameSettingsModal.vue` | `RunnerSettings.vue` |
|---|---|---|
| `outline` | `2px solid var(--l-accent)` | `2px solid var(--accent-focus)` |

Только в: `GameSettingsModal.vue`, `RunnerSettings.vue`.

Одинаково: `outline-offset: 2px`.

### `.settings-btn:hover`

Только в: `RunnerSettings.vue`, `SettingsModal.vue`, `UnavailableModal.vue`.

Одинаково: `background: rgba(255, 255, 255, 0.1)`.

### Только в одном файле

- `SettingsModal.vue` — `.diag__toolbar .settings-btn`: flex: none; white-space: nowrap
- `GameSettingsModal.vue` — `.game-settings__kb-window .game-settings__footer .settings-btn`: min-height: 40px; padding: 7px 17px
- `SettingsModal.vue` — `.proton__actions .settings-btn`: justify-content: flex-start; min-width: 0; min-height: 40px; padding: 8px 10px; font-size: 12.5px; line-height: 16px; text-align: left
- `SettingsModal.vue` — `.proton__actions .settings-btn i`: flex: none; width: 14px; text-align: center
- `RunnerSettings.vue` — `.settings-btn.danger`: box-shadow: inset 0 0 0 1px rgba(224, 69, 90, 0.45); color: #ff8796
- `RunnerSettings.vue` — `.settings-btn.danger:disabled:hover`: background: rgba(255, 255, 255, 0.05)
- `RunnerSettings.vue` — `.settings-btn.danger:hover`: background: rgba(224, 69, 90, 0.12)
- `GameSettingsModal.vue` — `.settings-btn.ghost:hover:not(:disabled)`: color: var(--l-text); background: rgba(255, 255, 255, 0.06)
- `GameSettingsModal.vue` — `.settings-btn.primary:active:not(:disabled)`: background: var(--accent-pressed)
- `GameSettingsModal.vue` — `.settings-btn.primary:hover:not(:disabled)`: background: var(--accent-hover); box-shadow: 0 6px 20px color-mix(in srgb, var(--l-accent) 35%, transparent)
- `GameSettingsModal.vue` — `.settings-btn:active:not(:disabled)`: transform: scale(0.97)
- `GameSettingsModal.vue` — `.settings-btn:hover:not(:disabled)`: background: rgba(255, 255, 255, 0.1)
- `GameSettingsModal.vue` — `.settings-btn[aria-busy="true"]`: cursor: progress
- `SettingsModal.vue` — `.settings-row > .settings-btn`: flex: none; width: max-content; max-width: 240px; text-align: center
- `RunnerSettings.vue` — `@media (prefers-reduced-motion: reduce) › .settings-btn`: transition: none

## settings-group — 2 файла

`components/Launcher/RunnerSettings.vue:739`, `components/Launcher/SettingsModal.vue:1332`

### `.settings-group`

| свойство | `RunnerSettings.vue` | `SettingsModal.vue` |
|---|---|---|
| `color` | `var(--l-text-dim)` | `var(--l-text-dim, rgba(255, 255, 255, 0.5))` |

Одинаково: `font-size: 12px`, `font-weight: 600`, `letter-spacing: 0.08em`, `line-height: 16px`, `margin: 32px 0 4px`, `text-transform: uppercase`.

### `.settings-group:first-child`

| свойство | `RunnerSettings.vue` | `SettingsModal.vue` |
|---|---|---|
| `margin-top` | `16px` | `0` |

Одинаковых свойств нет.

## settings-link — 2 файла

`components/Launcher/RunnerSettings.vue:792`, `components/Launcher/SettingsModal.vue:1399`

### `.settings-link`

| свойство | `RunnerSettings.vue` | `SettingsModal.vue` |
|---|---|---|
| `color` | `var(--l-accent)` | `var(--l-accent, #ff7a1c)` |
| `font-family` | `inherit` | — |
| `line-height` | `20px` | `1.4` |
| `text-underline-offset` | `3px` | — |

Одинаково: `align-self: flex-start`, `background: none`, `border: 0`, `cursor: pointer`, `font-size: 12px`, `margin-top: 2px`, `padding: 0`, `text-decoration: underline`.

### `.settings-link:focus-visible`

| свойство | `RunnerSettings.vue` | `SettingsModal.vue` |
|---|---|---|
| `outline` | `2px solid var(--l-accent)` | `2px solid var(--l-accent, #ff7a1c)` |

Одинаково: `outline-offset: 2px`.

### `.settings-link:hover`

Одинаково: `filter: brightness(1.15)`.

## settings-modal — 8 файлов

`assets/css/launcher.css:495`, `components/Launcher/ChangelogModal.vue:310`, `components/Launcher/GameNotesModal.vue:98`, `components/Launcher/GameSettingsModal.vue:1435`, `components/Launcher/SettingsModal.vue:1110`, `components/Launcher/SponsorsModal.vue:161`, `components/Launcher/UnavailableModal.vue:239`, `components/Launcher/WelcomeModal.vue:131`

### `.settings-modal`

Только в: `ChangelogModal.vue`, `GameNotesModal.vue`, `GameSettingsModal.vue`, `SettingsModal.vue`, `SponsorsModal.vue`, `UnavailableModal.vue`, `WelcomeModal.vue`.

Одинаково: `align-items: center`, `background: var(--l-modal-scrim)`, `display: flex`, `inset: 0`, `justify-content: center`, `position: absolute`, `z-index: 50`.

### `.settings-modal__close`

| свойство | `GameNotesModal.vue` | `SettingsModal.vue` | `UnavailableModal.vue` |
|---|---|---|---|
| `align-items` | — | `center` | — |
| `background` | `transparent` | `rgba(0, 0, 0, 0.32)` | `transparent` |
| `border-radius` | `8px` | `var(--l-radius-sm)` | `8px` |
| `box-shadow` | — | `inset 0 0 0 1px var(--l-border)` | — |
| `display` | — | `flex` | — |
| `font-size` | `17px` | `15px` | `17px` |
| `height` | `36px` | `40px` | `36px` |
| `justify-content` | — | `center` | — |
| `padding` | — | `0` | — |
| `position` | — | `absolute` | — |
| `right` | — | `32px` | — |
| `top` | — | `24px` | — |
| `width` | `36px` | `40px` | `36px` |
| `z-index` | — | `4` | — |

Только в: `GameNotesModal.vue`, `SettingsModal.vue`, `UnavailableModal.vue`.

Одинаково: `border: none`, `color: var(--l-text-dim)`, `cursor: pointer`, `transition: background var(--l-fast), color var(--l-fast)`.

### `.settings-modal__close:hover`

| свойство | `GameNotesModal.vue` | `SettingsModal.vue` | `UnavailableModal.vue` |
|---|---|---|---|
| `background` | `rgba(255, 255, 255, 0.08)` | `rgba(255, 255, 255, 0.1)` | `rgba(255, 255, 255, 0.08)` |
| `color` | `#fff` | `var(--l-text)` | `#fff` |

Только в: `GameNotesModal.vue`, `SettingsModal.vue`, `UnavailableModal.vue`.

Одинаковых свойств нет.

### `.settings-modal__footer`

| свойство | `GameSettingsModal.vue` | `SettingsModal.vue` |
|---|---|---|
| `padding` | `16px 32px 20px` | `16px 32px` |

Только в: `GameSettingsModal.vue`, `SettingsModal.vue`.

Одинаково: `align-items: center`, `box-shadow: inset 0 1px 0 var(--l-modal-border)`, `display: flex`, `flex: none`, `gap: 12px`.

### `.settings-modal__footer-spacer`

Только в: `GameSettingsModal.vue`, `SettingsModal.vue`.

Одинаково: `flex: 1`.

### `.settings-modal__header`

| свойство | `GameNotesModal.vue` | `UnavailableModal.vue` |
|---|---|---|
| `flex` | `none` | — |

Только в: `GameNotesModal.vue`, `UnavailableModal.vue`.

Одинаково: `align-items: center`, `border-bottom: 1px solid var(--l-border)`, `display: flex`, `justify-content: space-between`, `padding: 16px 18px 16px 24px`.

### `.settings-modal__title`

| свойство | `GameNotesModal.vue` | `SettingsModal.vue` | `UnavailableModal.vue` |
|---|---|---|---|
| `font-size` | `15px` | `28px` | `15px` |
| `font-weight` | `600` | `700` | `600` |
| `letter-spacing` | `0.09em` | `0.01em` | `0.09em` |
| `line-height` | — | `36px` | — |
| `margin` | — | `0` | — |
| `text-transform` | `uppercase` | — | `uppercase` |

Только в: `GameNotesModal.vue`, `SettingsModal.vue`, `UnavailableModal.vue`.

Одинаково: `color: var(--l-text)`.

### `.settings-modal__window`

| свойство | `ChangelogModal.vue` | `GameNotesModal.vue` | `GameSettingsModal.vue` | `SettingsModal.vue` | `SponsorsModal.vue` | `UnavailableModal.vue` | `WelcomeModal.vue` |
|---|---|---|---|---|---|---|---|
| `border` | `none` | `1px solid var(--l-modal-border)` | `none` | `none` | — | `1px solid var(--l-modal-border)` | — |
| `box-shadow` | `inset 0 0 0 1px var(--l-modal-border), 0 24px 72px rgba(0, 0, 0, 0.6)` | `0 24px 72px rgba(0, 0, 0, 0.6)` | `inset 0 0 0 1px var(--l-modal-border), 0 24px 72px rgba(0, 0, 0, 0.6)` | `inset 0 0 0 1px var(--l-modal-border), 0 24px 72px rgba(0, 0, 0, 0.6)` | `inset 0 0 0 1px var(--l-modal-border), 0 24px 72px rgba(0, 0, 0, 0.6)` | `0 24px 72px rgba(0, 0, 0, 0.6)` | `inset 0 0 0 1px var(--l-modal-border), 0 24px 72px rgba(0, 0, 0, 0.6)` |
| `height` | — | — | — | `min(720px, calc(100% - 64px))` | — | — | — |
| `overflow` | `hidden` | `hidden` | `hidden` | `hidden` | `hidden` | `hidden` | — |
| `position` | `relative` | — | `relative` | `relative` | — | — | — |
| `width` | — | — | — | `min(1040px, calc(100% - 64px))` | — | — | — |

Только в: `ChangelogModal.vue`, `GameNotesModal.vue`, `GameSettingsModal.vue`, `SettingsModal.vue`, `SponsorsModal.vue`, `UnavailableModal.vue`, `WelcomeModal.vue`.

Одинаково: `background: var(--l-modal-bg)`, `border-radius: var(--l-radius)`, `display: flex`, `flex-direction: column`.

### Только в одном файле

- `launcher.css` — `.launcher-root .settings-modal`: -webkit-app-region: no-drag
- `launcher.css` — `.launcher-root[data-theme="liquid-glass"] :is( .settings-modal__window, .profile__window, .confirm__window, .library-setup__window, .ls-launch__window, .friends )`: background-color: rgba(18, 22, 30, 0.58); background-image: linear-gradient(var(--lg-tint), var(--lg-tint)), var(--lg-sheen); -webkit-backdrop-filter: var(--lg-filter); backdrop-filter: var(--lg-filter); border-color: var(--l-modal-border); box-shadow: var(--lg-rim), var(--lg-shadow)
- `SettingsModal.vue` — `.settings-modal__badge`: flex: 0 0 88px; width: 88px; height: 88px; display: flex; align-items: center; justify-content: center; border-radius: 18px; background: radial-gradient(100% 100% at 30% 20%, color-mix(in srgb, var(--l-accent) 22%, transparent) 0%, transparent 70%), var(--l-modal-bg); box-shadow: inset 0 0 0 1px var(--l-border); color: var(--l-accent); font-size: 36px
- `SettingsModal.vue` — `.settings-modal__body`: position: relative; flex: 1 1 auto; min-height: 0; display: grid; grid-template: minmax(0, 1fr) / minmax(0, 1fr); overflow: hidden
- `SettingsModal.vue` — `.settings-modal__chip`: padding: 4px 10px; border-radius: var(--l-radius-sm); background: rgba(255, 255, 255, 0.05); color: var(--l-text-dim); font-size: 12.5px; font-variant-numeric: tabular-nums
- `GameNotesModal.vue` — `.settings-modal__close:active`: background: rgba(255, 255, 255, 0.12)
- `SettingsModal.vue` — `.settings-modal__close:focus-visible`: outline: 2px solid var(--l-accent); outline-offset: 2px
- `SettingsModal.vue` — `.settings-modal__hero`: position: relative; flex: none; padding: 0 32px; box-shadow: inset 0 -1px 0 var(--l-modal-border)
- `SettingsModal.vue` — `.settings-modal__identity`: position: relative; display: flex; align-items: center; gap: 24px; padding-top: 24px; padding-right: 56px
- `SettingsModal.vue` — `.settings-modal__meta`: display: flex; flex-wrap: wrap; align-items: center; gap: 6px 12px; color: var(--l-text-faint); font-size: 13px; line-height: 20px
- `SettingsModal.vue` — `.settings-modal__section`: grid-area: 1 / 1; min-height: 0; min-width: 0; box-sizing: border-box; padding: 28px 32px 32px; overflow-y: auto; overscroll-behavior: contain; display: flex; flex-direction: column; gap: 8px
- `SettingsModal.vue` — `.settings-modal__tab`: display: inline-flex; align-items: center; gap: 8px; height: 48px; padding: 0 12px; border: 0; background: none; color: var(--l-text-dim); font-family: inherit; font-size: 13.5px; font-weight: 600; line-height: 20px; white-space: nowrap; cursor: pointer; transition: color var(--l-fast)
- `SettingsModal.vue` — `.settings-modal__tab i`: font-size: 12px; color: var(--l-text-faint); transition: color var(--l-fast)
- `SettingsModal.vue` — `.settings-modal__tab-ink`: bottom: 0; height: 2px; border-radius: 2px 2px 0 0; background: var(--l-accent); box-shadow: 0 0 12px color-mix(in srgb, var(--l-accent) 60%, transparent)
- `SettingsModal.vue` — `.settings-modal__tab.is-active`: color: var(--l-text)
- `SettingsModal.vue` — `.settings-modal__tab.is-active i`: color: var(--l-accent)
- `SettingsModal.vue` — `.settings-modal__tab:focus-visible`: outline: 2px solid var(--l-accent); outline-offset: -4px; border-radius: var(--l-radius-sm)
- `SettingsModal.vue` — `.settings-modal__tab:hover`: color: var(--l-text)
- `SettingsModal.vue` — `.settings-modal__tabs`: position: relative; display: flex; gap: 8px; margin: 20px 0 0 -12px
- `SettingsModal.vue` — `.settings-modal__who`: flex: 1 1 auto; min-width: 0; display: flex; flex-direction: column; align-items: flex-start; gap: 8px

## settings-row — 3 файла

`components/Launcher/GameSettingsModal.vue:2028`, `components/Launcher/RunnerSettings.vue:753`, `components/Launcher/SettingsModal.vue:1346`

### `.settings-row`

Одинаково: `align-items: center`, `box-shadow: inset 0 -1px 0 rgba(255, 255, 255, 0.045)`, `display: flex`, `gap: 20px`, `justify-content: space-between`, `padding: 16px 0`.

### `.settings-row.column`

Одинаково: `align-items: stretch`, `flex-direction: column`, `gap: 12px`.

### `.settings-row:last-child`

Только в: `GameSettingsModal.vue`, `SettingsModal.vue`.

Одинаково: `box-shadow: none`.

### `.settings-row__hint`

Одинаково: `color: var(--l-text-faint)`, `font-size: 12px`, `line-height: 20px`.

### `.settings-row__label`

Одинаково: `color: var(--l-text)`, `font-size: 14px`, `font-weight: 500`, `line-height: 24px`.

### `.settings-row__text`

Одинаково: `display: flex`, `flex-direction: column`, `gap: 4px`, `min-width: 0`.

### Только в одном файле

- `SettingsModal.vue` — `.settings-row:first-child`: padding-top: 0
- `RunnerSettings.vue` — `.settings-row__hint.is-error`: color: #ff6b78
- `RunnerSettings.vue` — `@media (max-width: 760px) › .settings-row:not(.column)`: flex-direction: column; align-items: stretch

## settings-select — 3 файла

`components/Launcher/GameSettingsModal.vue:1994`, `components/Launcher/RunnerSettings.vue:850`, `components/Launcher/SettingsModal.vue:1429`

### `.settings-select`

| свойство | `GameSettingsModal.vue` | `RunnerSettings.vue` | `SettingsModal.vue` |
|---|---|---|---|
| `line-height` | `20px` | `20px` | — |
| `min-width` | `160px` | — | `150px` |

Одинаково: `align-items: center`, `background: rgba(255, 255, 255, 0.05)`, `border: 1px solid var(--l-border)`, `border-radius: var(--l-radius-sm)`, `color: var(--l-text)`, `display: flex`, `font-size: 13px`, `font-weight: 500`, `gap: 12px`, `justify-content: space-between`, `padding: 9px 14px`, `transition: background var(--l-fast), border-color var(--l-fast)`.

### `.settings-select i`

| свойство | `GameSettingsModal.vue` | `RunnerSettings.vue` | `SettingsModal.vue` |
|---|---|---|---|
| `transition` | `transform var(--l-med), color var(--l-fast)` | `transform var(--l-fast)` | `transform var(--l-fast)` |

Одинаково: `color: var(--l-text-dim)`, `font-size: 10px`.

### `.settings-select.open`

Одинаково: `background: rgba(255, 255, 255, 0.08)`, `border-color: var(--l-border-strong)`.

### `.settings-select.open i`

| свойство | `GameSettingsModal.vue` | `RunnerSettings.vue` | `SettingsModal.vue` |
|---|---|---|---|
| `color` | `var(--l-accent)` | — | — |

Одинаково: `transform: rotate(180deg)`.

### `.settings-select:hover`

Одинаково: `background: rgba(255, 255, 255, 0.08)`, `border-color: var(--l-border-strong)`.

## settings-switch — 4 файла

`components/Launcher/GameSettingsModal.vue:2067`, `components/Launcher/GhostingPersonalization.vue:661`, `components/Launcher/RunnerSettings.vue:893`, `components/Launcher/SettingsModal.vue:1544`

### `.settings-switch`

| свойство | `GameSettingsModal.vue` | `GhostingPersonalization.vue` | `RunnerSettings.vue` | `SettingsModal.vue` |
|---|---|---|---|---|
| `flex` | `none` | `none` | `none` | — |

Одинаково: `cursor: pointer`, `display: inline-flex`, `position: relative`.

### `.settings-switch input`

| свойство | `GameSettingsModal.vue` | `GhostingPersonalization.vue` | `RunnerSettings.vue` | `SettingsModal.vue` |
|---|---|---|---|---|
| `inset` | — | `0` | — | — |

Одинаково: `cursor: pointer`, `height: 100%`, `margin: 0`, `opacity: 0`, `position: absolute`, `width: 100%`.

### `.settings-switch input:checked + .settings-switch__track`

| свойство | `GameSettingsModal.vue` | `RunnerSettings.vue` | `SettingsModal.vue` |
|---|---|---|---|
| `box-shadow` | `0 0 12px color-mix(in srgb, var(--l-accent) 40%, transparent)` | — | — |

Только в: `GameSettingsModal.vue`, `RunnerSettings.vue`, `SettingsModal.vue`.

Одинаково: `background: var(--accent)`.

### `.settings-switch input:checked + .settings-switch__track .settings-switch__thumb`

Только в: `GameSettingsModal.vue`, `RunnerSettings.vue`, `SettingsModal.vue`.

Одинаково: `background: var(--l-switch-thumb-on, #fff)`, `transform: translateX(18px)`.

### `.settings-switch input:focus-visible + .settings-switch__track`

Только в: `GameSettingsModal.vue`, `RunnerSettings.vue`, `SettingsModal.vue`.

Одинаково: `outline: 2px solid var(--accent-focus)`, `outline-offset: 2px`.

### `.settings-switch__thumb`

| свойство | `GameSettingsModal.vue` | `GhostingPersonalization.vue` | `RunnerSettings.vue` | `SettingsModal.vue` |
|---|---|---|---|---|
| `border-radius` | `9px` | `50%` | `50%` | `50%` |
| `box-shadow` | `0 1px 3px rgba(0, 0, 0, 0.35)` | — | — | — |
| `transform` | `translateX(0)` | — | — | `translateX(0)` |
| `transition` | `transform var(--l-dur) var(--l-ease-spring), width var(--l-fast)` | `transform var(--l-med)` | `transform var(--l-med)` | `transform var(--l-med)` |

Одинаково: `background: #fff`, `height: 18px`, `width: 18px`.

### `.settings-switch__track`

| свойство | `GameSettingsModal.vue` | `GhostingPersonalization.vue` | `RunnerSettings.vue` | `SettingsModal.vue` |
|---|---|---|---|---|
| `box-sizing` | — | `border-box` | `border-box` | — |
| `transition` | `background var(--l-med), box-shadow var(--l-med)` | `background var(--l-med)` | `background var(--l-med)` | `background var(--l-med)` |

Одинаково: `align-items: center`, `background: rgba(255, 255, 255, 0.12)`, `border-radius: 12px`, `display: flex`, `height: 24px`, `padding: 3px`, `width: 42px`.

### `@media (prefers-reduced-motion: reduce) › .settings-switch__thumb`

| свойство | `GameSettingsModal.vue` | `RunnerSettings.vue` |
|---|---|---|
| `animation` | `none` | — |

Только в: `GameSettingsModal.vue`, `RunnerSettings.vue`.

Одинаково: `transition: none`.

### Только в одном файле

- `RunnerSettings.vue` — `.settings-switch input:disabled + .settings-switch__track`: opacity: 0.45
- `GameSettingsModal.vue` — `.settings-switch:active .settings-switch__thumb`: width: 22px
- `GameSettingsModal.vue` — `.settings-switch:active input:checked + .settings-switch__track .settings-switch__thumb`: transform: translateX(14px)
- `GameSettingsModal.vue` — `.settings-switch:hover input:not(:checked) + .settings-switch__track`: background: rgba(255, 255, 255, 0.18)
- `GhostingPersonalization.vue` — `@media (prefers-reduced-motion: reduce) › .settings-switch *`: transition: none
- `RunnerSettings.vue` — `@media (prefers-reduced-motion: reduce) › .settings-switch__track`: transition: none
- `GhostingPersonalization.vue` — `input:checked + .settings-switch__track`: background: var(--accent)
- `GhostingPersonalization.vue` — `input:checked + .settings-switch__track .settings-switch__thumb`: background: var(--l-switch-thumb-on, #fff); transform: translateX(18px)
- `GhostingPersonalization.vue` — `input:focus-visible + .settings-switch__track`: outline: 2px solid var(--accent-focus); outline-offset: 2px
