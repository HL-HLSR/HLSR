# gpui-pre-wgpu 0.3.13 — локальный патч HLSR

Копия пакета `gpui-pre-wgpu` 0.3.13 с crates.io с одной правкой: нативный рендерер пробует
бэкенд `wgpu::Backends::VULKAN` отдельно и подгружает `wgpu::Backends::GL` только при отказе
Vulkan. Подключается из корневого `Cargo.toml` (`[patch.crates-io]`).

## Зачем

Исходный `WgpuContext::instance` создавал `wgpu::Instance` сразу с `Backends::VULKAN | Backends::GL`.
На Linux это при каждом запуске подгружало EGL/GLX и Mesa `libgallium`/`libLLVM.so` (~40–46 МиБ
RSS + ~3,6 МиБ кучи EGL-драйвера), даже когда окно полностью работает на аппаратном Vulkan.

## Что сделано (все места помечены `HLSR:`)

- `src/wgpu_context.rs`: добавлен `WgpuContext::instance_with_backends(backends, display)`.
- `src/wgpu_renderer.rs`: `WgpuRenderer::new` и восстановление после потери устройства пробуют
  `Backends::VULKAN` первым и переходят на `Backends::GL` лишь при ошибке инициализации Vulkan.
- `src/wgpu_renderer.rs`, `src/shaders.wgsl`: финальный проход `window_corner_mask` рисует
  сглаженную альфа-маску `saturate(0.5 - distance)` по 4 углам окна, когда
  `scene.window_corner_radius > 0`, устраняя лесенки от накопления альфы нескольких наложенных
  скруглённых слоёв.
- `src/shaders.wgsl`:
  - `vs_poly_sprite` / `fs_poly_sprite` зажимают UV в пределах центров крайних текселей тайла
    (`tile_uv_bounds`), чтобы при билинейном растягивании спрайта (`Radial` 64×64) не протекали
    пиксели соседних тайлов полихромного атласа;
  - `vs_quad` расширяет прямоугольник растеризации фигурных квадов (`is_shaped`) на `0.5 px` (с
    обрезкой `content_mask`), чтобы дробный скролл и правило top-left не срезали нижний/правый
    край рамки; `fs_quad` считает `inner_sdf` равной рамки по точной формуле
    `-(outer_sdf + reduced_border.x)` без скачка в `-1.0` на стыке прямой стороны и дуги угла;
  - `vs_shadow` так же расширяет прямоугольник растеризации на `0.5 px` для внутренних теней и
    теней без размытия (`inset != 0 || blur_radius == 0`, которыми рисуется `ring()` у модалок,
    меню и тостов), чтобы дробные координаты не срезали внешнее сглаживание контура.
- `src/cosmic_text_system.rs`:
  - шрифты, явно загруженные приложением через `add_fonts` (`Font Awesome 7 Brands`, `Regular`,
    `HLSR Symbols`), и известные цветные emoji-шрифты больше не отбрасываются фильтром
    `charmap().map('m') == 0`;
  - для emoji-графем (Unicode Emoji Presentation и `U+FE0F`) без глифа в основном шрифте
    подставляется системный цветной emoji-шрифт (`Noto Color Emoji` и аналоги) раньше ч/б
    текстовых шрифтов (`DejaVu Sans` / `Noto Sans Symbols 2`).
