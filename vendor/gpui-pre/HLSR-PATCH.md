# gpui-pre 0.3.13 — локальный патч HLSR

Копия пакета `gpui-pre` 0.3.13 с crates.io (Zed `gpui`) с правкой текста: разрядка (CSS
`letter-spacing`) и перенос с висящим пробелом, как у Chromium. Подключается из корневого
`Cargo.toml` (`[patch.crates-io]`); в workspace не входит (`exclude = ["vendor"]`), поэтому clippy
и тесты workspace его не трогают. Исходная копия — отдельный коммит без правок (без
`examples/image/black-cat-typing.gif`, 4 МБ: примеры не собираются), дифф патча — следующий.
Позже убраны и `examples/gif_viewer.rs` (пример к той gif, и его `[[example]]` в `Cargo.toml`), и
`Cargo.lock` пакета: у зависимости через `[patch]` он не читается.

## Зачем

- `body { letter-spacing: .01em }` 0.4.9 даёт всему унаследованному тексту 0,14 CSS px после
  каждого знака. В GPUI разрядки не было: строки уже, переносы другие (лента «Главной», описание
  спонсорства — 605 px вместо 615).
- Перенос GPUI сравнивал с шириной блока строку вместе с пробелом в её конце. Chromium пробел в
  конце строки не считает (он висит за краем), поэтому у GPUI слово уходило на следующую строку
  раньше («Файлы лаунчера отличаются от / официальной…» в уведомлении шириной 268 px).

## Что сделано (все места помечены `HLSR:`)

- `src/style.rs`: `TextStyle::letter_spacing: AbsoluteLength` (наследуется через
  `TextStyleRefinement`, по умолчанию 0); `to_run(len, rem_size)` переводит её в пиксели.
- `src/styled.rs`: `Styled::letter_spacing(…)`.
- `src/text_system.rs`: `TextRun::letter_spacing: Pixels`. `shape_text` и `layout_line` собирают
  куски разрядки (байты, шаг) рядом с `FontRun` (сам `FontRun` не менялся — его строят
  `gpui-pre-wgpu` и `gpui-pre-windows`); строка без разрядки даёт пустой список.
- `src/text_system/line_layout.rs`:
  - куски разрядки — часть ключа кэша раскладки; `LineLayoutCache::layout_line` и
    `layout_wrapped_line` берут их параметром;
  - `apply_letter_spacing`: после раскладки платформой каждый кластер (смена `glyph.index`)
    сдвигается на сумму шагов перед ним, ширина строки растёт на шаг каждого кластера, и
    последнего тоже (так делает Chromium). Перенос, отрисовка, `index_for_x` берут уже сдвинутые
    глифы;
  - `compute_wrap_boundaries`: пробел переноса не вызывает — он висит в конце строки.
- `src/text_system/line_wrapper.rs`: обрезка (`truncate_line`, `truncate_wrapped_line`) прибавляет
  к ширине каждого знака разрядку строки (`run_spacing`: одна на все куски, иначе
  `debug_assert`); в `truncate_wrapped_line` пробел тоже
  висит, как в `compute_wrap_boundaries`. `wrap_line` (мягкий перенос редактора Zed) не менялся.
- `src/elements/text.rs`, `src/elements/div.rs`: вызовы `to_run` с `rem_size` окна;
  `StyledText::with_default_highlights` окна не знает — разрядку в rem там считает от rem 16 px.
- `src/scene.rs`, `src/bounds_tree.rs`, `src/path_winding.rs`, `src/window.rs`:
  - `Window::with_paint_transform` и `Window::paint_transform_bounds` — преобразование примитивов
    сцены при отрисовке поддерева (сдвиг, равномерный масштаб вокруг точки опоры, прозрачность) с
    сохранением исходных примитивов в `paint_operations` (переходы `l-slide-*`, `l-pop`,
    `notif-flow`, `press` идут на частоте монитора без перерастеризации шрифтов и срыва кэша видов
    `Entity::cached`).
  - `BoundsTree::reserve_range` и быстрый `Scene::replay` — при повторе закэшированного вида
    (включая свайп с `PaintTransform`) сохраняются локальные относительные `DrawOrder` примитивов,
    а в `BoundsTree` делается одна запись на весь вид вместо O(N) вставок по каждому примитиву.
  - `Scene::window_corner_radius` и `Window::set_window_corner_radius` — передают рендереру радиус
    скругления окна в физических px для однопроходной сглаженной альфа-маски углов.
  - `src/view.rs`, `src/window.rs`: `Window::draw_generation` и `ViewElementState::last_painted_frame`
    заменяют безусловный сброс `window.refreshing = true` при перерисовке родительского
    `Entity::cached` — вложенный `Entity::cached` (разделы `GameSettings` внутри `GameConfigsPanel`,
    вкладки `ProfileModal`, превью внутри `GamePersonalization`) переиспользует свой кэш, пока его
    `prepaint_range`/`paint_range` записаны ровно в предыдущем кадре (`draw_generation - 1`).

## Шаг кадров окна

`src/platform.rs`, `frame_divisor(refresh)`: через сколько кадров монитора рисует окно. Сейчас —
1: окно рисует каждый кадр монитора, как rAF Chromium в 0.4.9 (решение владельца 22.11; прежний
предел 60 к/с «как у Discord» от 19.11 снят — на 120+ Гц лаунчер был заметно менее плавным).
Функция оставлена единой точкой: её берут поток vsync `gpui-pre-windows` (`vsync.rs`: будит окна
каждым k-м vblank) и сетка движения лаунчера (`crates/hlsr-gpui/src/motion/driver.rs`; при
неизвестной частоте монитора — X11 — сетки нет, кадр на каждый будильник платформы). Тест — там
же, `motion::driver::pacing` (`cargo test -p hlsr-gpui --lib pacing`): тесты самого пакета вне
workspace не собираются.

Остальные файлы — как в пакете crates.io (`Cargo.toml` нормализован пакетом, `Cargo.toml.orig`,
`LICENSE-APACHE`).

## Чего нет

- Лигатуры при разрядке не отключаются (Chromium их выключает); у Rubik в кириллице и латинице
  интерфейса это не видно.
- Выравнивание по центру и справа у перенесённой строки считает ширину вместе с висящим
  пробелом (так было и до патча): строка сдвинута на половину пробела.
- Обрезка (`line_wrapper.rs`) не знает разной разрядки кусков одной строки: шаг — один на всю
  строку, как и шрифт обёртки. Из стилей такой строки не получить (`HighlightStyle` разрядку не
  меняет).
- `apply_letter_spacing` идёт по глифам в порядке показа и берёт шаг по `glyph.index`: у
  справа-налево (bidi) шаги легли бы не с той стороны знака. В интерфейсе RTL нет.
- `force_width` (сетка терминала Zed) и разрядка вместе не проверены: разрядка ложится поверх
  выровненных по сетке глифов. В HLSR `force_width` никто не передаёт.
- Тесты пакета не запускаются (пакет вне workspace); поведение проверяет
  `crates/hlsr-gpui/src/text.rs` (`letter_spacing_and_hanging_space`).

## Обновление GPUI

Все `gpui-pre-*` обновляются разом (точный пин в `Cargo.toml`). При обновлении: новая копия
пакета отдельным коммитом, затем перенос правок `HLSR:` по этому списку и проверка
`cargo test -p hlsr-gpui text::`.
