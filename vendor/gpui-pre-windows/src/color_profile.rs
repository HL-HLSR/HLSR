//! HLSR: цвета по профилю монитора окна, как в Chromium 0.4.9 (`HLSR-PATCH.md`).
//!
//! Chromium читает ICC-профиль монитора (`GetICMProfileW` по устройству монитора,
//! `ui/display/win/color_profile_reader.cc`) и переводит кадр из sRGB в него (`ui/gfx/icc_profile.cc`,
//! skcms): sRGB → линейный → XYZ D50 → линейный RGB монитора → обратная кривая монитора. Здесь то
//! же делает последний проход кадра (`ColorPass` в `directx_renderer.rs`, `color_fragment` в
//! `shaders.hlsl`). Профиль, который в 8 битах ничего не меняет (sRGB Windows), прохода не включает.

use windows::Win32::Graphics::Gdi::{CreateDCW, DeleteDC, HDC, HMONITOR};
use windows::core::{PCWSTR, w};

/// Точек в таблице обратной кривой (на [0, 1], между ними — линейно).
pub(crate) const CURVE_POINTS: usize = 4096;

/// sRGB → XYZ D50 (skcms `skcms_sRGB_profile`, его берёт Chromium).
const SRGB_TO_XYZ_D50: [[f64; 3]; 3] = [
    [0.436065674, 0.385147095, 0.143066406],
    [0.222488403, 0.716873169, 0.060607910],
    [0.013916016, 0.097076416, 0.714096069],
];

// Своя привязка: в пакете `windows` у GPUI не включён `Win32_UI_ColorSystem`.
windows_core::link!("gdi32.dll" "system" fn GetICMProfileW(hdc: HDC, size: *mut u32, path: *mut u16) -> windows_core::BOOL);

/// Счётчик событий, на которых Chromium перечитывает пути профилей (`ScreenWin::OnWndProc` →
/// `ColorProfileReader::UpdateIfNeeded`): смена дисплеев, активация приложения, рабочая область.
static DISPLAYS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub(crate) fn displays_changed() {
    DISPLAYS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn displays_epoch() -> u32 {
    DISPLAYS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Перевод sRGB → RGB монитора: матрица и обратные кривые монитора.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ColorTransform {
    /// Линейный sRGB → линейный RGB монитора, по строкам.
    pub(crate) matrix: [[f32; 3]; 3],
    /// Обратные кривые каналов r, g, b: `CURVE_POINTS` значений на [0, 1].
    pub(crate) curves: [Vec<f32>; 3],
}

impl ColorTransform {
    /// Перевод в профиль из файла `path` (читается в фоне, как у Chromium). `None` — файла нет, он
    /// не матричный, белая точка не D50 (так же отбрасывает Chromium) или перевод ничего не меняет.
    pub(crate) fn load(path: &str) -> Option<Self> {
        let transform = std::fs::read(path)
            .inspect_err(|err| log::error!("color profile {path}: {err}"))
            .ok()
            .and_then(|data| Self::from_icc(&data))
            .filter(|transform| !transform.changes_nothing());
        log::info!(
            "color profile {path}: {}",
            if transform.is_some() { "on" } else { "off" }
        );
        transform
    }

    /// Матричный RGB-профиль ICC v2/v4 (`rXYZ`/`gXYZ`/`bXYZ` и `rTRC`/`gTRC`/`bTRC`).
    pub(crate) fn from_icc(data: &[u8]) -> Option<Self> {
        if data.get(16..20)? != b"RGB " || data.get(20..24)? != b"XYZ " {
            return None;
        }
        let count = be_u32(data, 128)? as usize;
        let tag = |sig: &[u8; 4]| {
            (0..count.min(1024)).find_map(|index| {
                let at = 132 + 12 * index;
                if data.get(at..at + 4)? != sig {
                    return None;
                }
                let start = be_u32(data, at + 4)? as usize;
                data.get(start..start.checked_add(be_u32(data, at + 8)? as usize)?)
            })
        };
        let xyz = |sig| {
            let tag = tag(sig)?;
            (tag.get(..4)? == b"XYZ ").then_some(())?;
            Some([s15(tag, 8)?, s15(tag, 12)?, s15(tag, 16)?])
        };
        let [r, g, b] = [xyz(b"rXYZ")?, xyz(b"gXYZ")?, xyz(b"bXYZ")?];
        let to_xyz: [[f64; 3]; 3] = std::array::from_fn(|row| [r[row], g[row], b[row]]);
        // Chromium отбрасывает профили с белой точкой не D50 (crbug.com/847024).
        let white = to_xyz.map(|row| row.iter().sum::<f64>());
        if (white[0] - 0.964).abs() > 0.04
            || (white[1] - 1.0).abs() > 0.04
            || (white[2] - 0.825).abs() > 0.04
        {
            return None;
        }
        let to_monitor = invert(to_xyz)?;
        let matrix = std::array::from_fn(|row| {
            std::array::from_fn(|column| {
                (0..3)
                    .map(|k| to_monitor[row][k] * SRGB_TO_XYZ_D50[k][column])
                    .sum::<f64>() as f32
            })
        });
        let [r, g, b] = [b"rTRC", b"gTRC", b"bTRC"].map(|sig| tag(sig).and_then(Curve::parse));
        Some(Self {
            matrix,
            curves: [r?.inverse(), g?.inverse(), b?.inverse()],
        })
    }

    /// Цвет sRGB в RGB монитора — то же, что `color_fragment` (проверка и тесты).
    pub(crate) fn apply(&self, rgb: [u8; 3]) -> [u8; 3] {
        let linear = rgb.map(|c| srgb_decode(c as f32 / 255.0));
        std::array::from_fn(|channel| {
            let row = self.matrix[channel];
            let value =
                (row[0] * linear[0] + row[1] * linear[1] + row[2] * linear[2]).clamp(0.0, 1.0);
            (lookup(&self.curves[channel], value) * 255.0).round() as u8
        })
    }

    /// Перевод не меняет ни одного цвета сетки больше чем на 1 из 255 (профиль sRGB).
    pub(crate) fn changes_nothing(&self) -> bool {
        let steps = (0..=255u8).step_by(15);
        steps.clone().all(|r| {
            steps.clone().all(|g| {
                steps.clone().all(|b| {
                    let out = self.apply([r, g, b]);
                    [r, g, b].iter().zip(out).all(|(&a, b)| a.abs_diff(b) <= 1)
                })
            })
        })
    }
}

/// Путь профиля монитора, как его отдаёт Windows Chromium (`GetICMProfileW` по DC устройства).
pub(crate) fn profile_path(monitor: HMONITOR) -> Option<String> {
    let info = crate::get_monitor_info(monitor).ok()?;
    let mut path = [0u16; 1024];
    let mut size = path.len() as u32;
    // SAFETY: `szDevice` — имя устройства с нулём в конце; DC удаляется тут же; `path` вмещает
    // `size` знаков.
    let found = unsafe {
        let dc = CreateDCW(
            w!("DISPLAY"),
            PCWSTR(info.szDevice.as_ptr()),
            PCWSTR::null(),
            None,
        );
        if dc.is_invalid() {
            return None;
        }
        let found = GetICMProfileW(dc, &mut size, path.as_mut_ptr());
        let _ = DeleteDC(dc);
        found
    };
    if !found.as_bool() {
        return None;
    }
    let len = path.iter().position(|&c| c == 0).unwrap_or(path.len());
    Some(String::from_utf16_lossy(&path[..len]))
}

/// Кривая канала: таблица или параметрическая `X >= d ? (aX + b)^g + e : cX + f` (ICC `para`).
enum Curve {
    Table(Vec<f64>),
    Para([f64; 7]),
}

impl Curve {
    fn power(gamma: f64) -> Self {
        Self::Para([gamma, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0])
    }

    fn parse(tag: &[u8]) -> Option<Self> {
        match tag.get(..4)? {
            b"curv" => match be_u32(tag, 8)? {
                0 => Some(Self::power(1.0)),
                1 => {
                    let gamma = u16::from_be_bytes(tag.get(12..14)?.try_into().ok()?);
                    Some(Self::power(gamma as f64 / 256.0))
                }
                count => (0..count as usize)
                    .map(|i| {
                        let at = 12 + 2 * i;
                        Some(
                            u16::from_be_bytes(tag.get(at..at + 2)?.try_into().ok()?) as f64
                                / 65535.0,
                        )
                    })
                    .collect::<Option<Vec<_>>>()
                    .map(Self::Table),
            },
            b"para" => {
                let kind = u16::from_be_bytes(tag.get(8..10)?.try_into().ok()?);
                let count = [1, 3, 4, 5, 7].get(kind as usize)?;
                let p: Vec<f64> = (0..*count)
                    .map(|i| s15(tag, 12 + 4 * i))
                    .collect::<Option<_>>()?;
                let (g, a, b) = (
                    p[0],
                    p.get(1).copied().unwrap_or(1.0),
                    p.get(2).copied().unwrap_or(0.0),
                );
                // Типы 1 и 2: ниже -b/a — 0 (или c), выше — степень (плюс c).
                let cut = if a != 0.0 { -b / a } else { 0.0 };
                Some(match kind {
                    0 => Self::power(g),
                    1 => Self::Para([g, a, b, 0.0, cut, 0.0, 0.0]),
                    2 => Self::Para([g, a, b, 0.0, cut, p[3], p[3]]),
                    3 => Self::Para([g, a, b, p[3], p[4], 0.0, 0.0]),
                    _ => Self::Para([g, a, b, p[3], p[4], p[5], p[6]]),
                })
            }
            _ => None,
        }
    }

    fn eval(&self, x: f64) -> f64 {
        match self {
            Self::Table(table) => {
                let at = x * (table.len() - 1) as f64;
                let index = (at as usize).min(table.len().saturating_sub(2));
                let next = table.get(index + 1).unwrap_or(&table[index]);
                table[index] + (next - table[index]) * (at - index as f64)
            }
            &Self::Para([g, a, b, c, d, e, f]) => {
                if x >= d {
                    (a * x + b).max(0.0).powf(g) + e
                } else {
                    c * x + f
                }
            }
        }
    }

    /// Обратная кривая: для каждой точки — x, при котором кривая доходит до неё (деление пополам;
    /// кривые профилей монитора не убывают).
    fn inverse(&self) -> Vec<f32> {
        (0..CURVE_POINTS)
            .map(|point| {
                let y = point as f64 / (CURVE_POINTS - 1) as f64;
                let (mut low, mut high) = (0.0, 1.0);
                for _ in 0..30 {
                    let mid = (low + high) / 2.0;
                    if self.eval(mid) < y {
                        low = mid;
                    } else {
                        high = mid;
                    }
                }
                ((low + high) / 2.0) as f32
            })
            .collect()
    }
}

fn srgb_decode(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn lookup(curve: &[f32], value: f32) -> f32 {
    let at = value * (curve.len() - 1) as f32;
    let index = (at as usize).min(curve.len() - 2);
    curve[index] + (curve[index + 1] - curve[index]) * (at - index as f32)
}

fn be_u32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

fn s15(data: &[u8], at: usize) -> Option<f64> {
    Some(i32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?) as f64 / 65536.0)
}

fn invert(m: [[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let cofactor = |r: usize, c: usize| {
        let (r0, r1, c0, c1) = ((r + 1) % 3, (r + 2) % 3, (c + 1) % 3, (c + 2) % 3);
        m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0]
    };
    let det = (0..3).map(|c| m[0][c] * cofactor(0, c)).sum::<f64>();
    (det.abs() > 1e-9)
        .then(|| std::array::from_fn(|r| std::array::from_fn(|c| cofactor(c, r) / det)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ColorPass, DirectXDevices};
    use windows::Win32::Graphics::{Direct3D11::*, Dxgi::Common::*};

    const COLOR: &str = r"C:\Windows\System32\spool\drivers\color";

    fn profile(name: &str) -> Option<ColorTransform> {
        ColorTransform::from_icc(&std::fs::read(format!(r"{COLOR}\{name}")).ok()?)
    }

    /// Профиль DISPLAY1 владельца; есть только на его машине.
    fn dell() -> Option<ColorTransform> {
        let dell = profile("AW2723DF.icm");
        if dell.is_none() {
            eprintln!("нет AW2723DF.icm — проверка чисел Chromium пропущена");
        }
        dell
    }

    /// Chromium 0.4.9 на DISPLAY1 (AW2723DF), `docs/gpui/reports/real-fix-shell.md`.
    const CHROMIUM_DELL: [([u8; 3], [u8; 3]); 6] = [
        ([219, 97, 162], [206, 105, 160]),
        ([95, 209, 127], [121, 206, 133]),
        ([0, 171, 255], [78, 168, 250]),
        ([17, 39, 75], [24, 38, 73]),
        ([232, 236, 241], [233, 236, 241]),
        ([10, 132, 255], [66, 130, 249]),
    ];

    #[test]
    fn dell_profile_gives_chromium_numbers() {
        let Some(dell) = dell() else { return };
        for (srgb, chromium) in CHROMIUM_DELL {
            assert_eq!(dell.apply(srgb), chromium, "{srgb:?}");
        }
        assert!(!dell.changes_nothing());
    }

    #[test]
    fn srgb_profile_changes_nothing_and_wide_ones_do() {
        assert!(
            profile("sRGB Color Space Profile.icm")
                .unwrap()
                .changes_nothing()
        );
        // Adobe RGB: `curv` с одной степенью. Ставит не Windows, а программы Adobe.
        if let Some(adobe) = profile("AdobeRGB1998.icc") {
            assert!(!adobe.changes_nothing());
            assert_eq!(adobe.apply([0, 0, 0]), [0, 0, 0]);
            assert_eq!(adobe.apply([255, 255, 255]), [255, 255, 255]);
        }
        // Не RGB и не матричные — без перевода.
        assert!(ColorTransform::from_icc(b"short").is_none());
        assert!(profile("CoatedFOGRA39.icc").is_none());
    }

    #[test]
    fn para_curves_follow_icc_formulas() {
        let tag = |kind: u16, params: &[f64]| {
            let mut tag = b"para\0\0\0\0".to_vec();
            tag.extend(kind.to_be_bytes());
            tag.extend([0, 0]);
            for p in params {
                tag.extend((((p * 65536.0).round()) as i32).to_be_bytes());
            }
            Curve::parse(&tag).unwrap()
        };
        let srgb = tag(3, &[2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045]);
        for x in [0.0, 0.02, 0.04045, 0.3, 0.7, 1.0] {
            assert!(
                (srgb.eval(x) - srgb_decode(x as f32) as f64).abs() < 1e-4,
                "{x}"
            );
        }
        let inverse = srgb.inverse();
        assert!((lookup(&inverse, 0.2) - 0.4845).abs() < 1e-3);
        // Тип 2: ниже -b/a — c.
        assert!((tag(2, &[2.0, 1.0, -0.5, 0.25]).eval(0.1) - 0.25).abs() < 1e-4);
    }

    /// Профили мониторов этой машины через `GetICMProfileW`: у монитора с профилем sRGB Windows
    /// перевода нет; время чтения (в фоне, кадр его не ждёт).
    #[test]
    fn monitor_profiles_load() {
        for monitor in crate::available_monitors() {
            let started = std::time::Instant::now();
            let path = profile_path(monitor).unwrap_or_default();
            let transform = ColorTransform::load(&path);
            let elapsed = started.elapsed();
            eprintln!(
                "{path}: перевод {}, {:.1} мс",
                if transform.is_some() {
                    "есть"
                } else {
                    "нет"
                },
                elapsed.as_secs_f64() * 1000.0
            );
            if path.ends_with("sRGB Color Space Profile.icm") {
                assert!(transform.is_none());
            }
        }
    }

    fn texture(
        device: &ID3D11Device,
        (width, height): (u32, u32),
        usage: D3D11_USAGE,
        bind: u32,
        pixels: Option<&[[u8; 4]]>,
    ) -> ID3D11Texture2D {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: usage,
            BindFlags: bind,
            CPUAccessFlags: if usage == D3D11_USAGE_STAGING {
                D3D11_CPU_ACCESS_READ.0 as u32
            } else {
                0
            },
            MiscFlags: 0,
        };
        let data = pixels.map(|pixels| D3D11_SUBRESOURCE_DATA {
            pSysMem: pixels.as_ptr().cast(),
            SysMemPitch: width * 4,
            SysMemSlicePitch: 0,
        });
        let mut texture = None;
        unsafe {
            device
                .CreateTexture2D(
                    &desc,
                    data.as_ref().map(|d| d as *const _),
                    Some(&mut texture),
                )
                .unwrap()
        };
        texture.unwrap()
    }

    struct Pass {
        frame: Option<ID3D11ShaderResourceView>,
        target: ID3D11Texture2D,
        target_view: Option<ID3D11RenderTargetView>,
        viewport: D3D11_VIEWPORT,
    }

    fn pass_over(device: &ID3D11Device, size: (u32, u32), pixels: Option<&[[u8; 4]]>) -> Pass {
        let rt = D3D11_BIND_RENDER_TARGET.0 as u32;
        let frame = texture(
            device,
            size,
            D3D11_USAGE_DEFAULT,
            rt | D3D11_BIND_SHADER_RESOURCE.0 as u32,
            pixels,
        );
        let target = texture(device, size, D3D11_USAGE_DEFAULT, rt, None);
        let (mut frame_view, mut target_view) = (None, None);
        unsafe {
            device
                .CreateShaderResourceView(&frame, None, Some(&mut frame_view))
                .unwrap();
            device
                .CreateRenderTargetView(&target, None, Some(&mut target_view))
                .unwrap();
        }
        Pass {
            frame: frame_view,
            target,
            target_view,
            viewport: D3D11_VIEWPORT {
                Width: size.0 as f32,
                Height: size.1 as f32,
                MaxDepth: 1.0,
                ..Default::default()
            },
        }
    }

    /// Проход на настоящей видеокарте: те же числа, что `apply` (и Chromium), прозрачное не
    /// трогает; цена прохода кадра 1600 × 900 и 2560 × 1440 — по меткам времени видеокарты (только
    /// в вывод: порог зависит от видеокарты).
    #[test]
    fn gpu_pass_matches_cpu_and_costs_little() {
        let dell = dell();
        let is_dell = dell.is_some();
        let Some(transform) = dell.or_else(|| profile("AdobeRGB1998.icc")) else {
            eprintln!("нет AW2723DF.icm и AdobeRGB1998.icc — проход не проверен");
            return;
        };
        let Ok(devices) = DirectXDevices::new() else {
            eprintln!("нет устройства D3D11 — проход не проверен");
            return;
        };
        let (device, context) = (&devices.device, &devices.device_context);
        let color_pass = ColorPass::new(device, &transform).unwrap();

        // Сетка цветов через 17, серые, числа Chromium; в конце — прозрачный и полупрозрачный.
        let steps: Vec<u8> = (0..=255u8).step_by(17).collect();
        let mut colors: Vec<[u8; 3]> = Vec::new();
        for &r in &steps {
            for &g in &steps {
                colors.extend(steps.iter().map(|&b| [r, g, b]));
            }
        }
        colors.extend((0..=255u8).map(|v| [v, v, v]));
        colors.extend(CHROMIUM_DELL.map(|(srgb, _)| srgb));
        let mut pixels: Vec<[u8; 4]> = colors.iter().map(|&[r, g, b]| [b, g, r, 255]).collect();
        pixels.push([0, 0, 0, 0]);
        pixels.push([40, 30, 110, 128]);
        let width = 256;
        let height = pixels.len().div_ceil(width) as u32;
        pixels.resize(width * height as usize, [0, 0, 0, 0]);
        let width = width as u32;

        let pass = pass_over(device, (width, height), Some(&pixels));
        color_pass.draw(context, &pass.frame, &pass.target_view, &pass.viewport);
        let staging = texture(device, (width, height), D3D11_USAGE_STAGING, 0, None);
        let mut out = vec![[0u8; 4]; pixels.len()];
        unsafe {
            context.CopyResource(&staging, &pass.target);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .unwrap();
            for y in 0..height as usize {
                std::ptr::copy_nonoverlapping(
                    (mapped.pData as *const u8).add(y * mapped.RowPitch as usize),
                    out[y * width as usize..].as_mut_ptr().cast(),
                    width as usize * 4,
                );
            }
            context.Unmap(&staging, 0);
        }

        for (index, &srgb) in colors.iter().enumerate() {
            let [b, g, r, a] = out[index];
            assert_eq!(([r, g, b], a), (transform.apply(srgb), 255), "{srgb:?}");
        }
        let chromium = colors.len() - CHROMIUM_DELL.len();
        if is_dell {
            for (index, (_, want)) in CHROMIUM_DELL.iter().enumerate() {
                let [b, g, r, _] = out[chromium + index];
                assert_eq!([r, g, b], *want);
            }
        }
        assert_eq!(out[colors.len()], [0, 0, 0, 0]);
        let [b, g, r, a] = out[colors.len() + 1];
        assert_eq!(a, 128);
        // Полупрозрачный: перевод цвета без умножения на альфу, затем умножение.
        let [er, eg, eb] = transform.apply([219, 60, 80]);
        for (got, want) in [r, g, b].into_iter().zip([er, eg, eb]) {
            assert!(
                (got as i32 - (want as i32 * 128 + 127) / 255).abs() <= 2,
                "{got} {want}"
            );
        }

        for size in [(1600, 900), (2560, 1440)] {
            let pass = pass_over(device, size, None);
            let query = |kind| {
                let mut query = None;
                unsafe {
                    device
                        .CreateQuery(
                            &D3D11_QUERY_DESC {
                                Query: kind,
                                MiscFlags: 0,
                            },
                            Some(&mut query),
                        )
                        .unwrap()
                };
                query.unwrap()
            };
            let (disjoint, start, end) = (
                query(D3D11_QUERY_TIMESTAMP_DISJOINT),
                query(D3D11_QUERY_TIMESTAMP),
                query(D3D11_QUERY_TIMESTAMP),
            );
            const RUNS: u32 = 200;
            for _ in 0..20 {
                color_pass.draw(context, &pass.frame, &pass.target_view, &pass.viewport);
            }
            unsafe {
                context.Begin(&disjoint);
                context.End(&start);
                for _ in 0..RUNS {
                    color_pass.draw(context, &pass.frame, &pass.target_view, &pass.viewport);
                }
                context.End(&end);
                context.End(&disjoint);
                context.Flush();
            }
            // Ответ ещё не готов — данные не пишутся (S_FALSE): ждём, пока поле не изменится.
            let mut info = D3D11_QUERY_DATA_TIMESTAMP_DISJOINT::default();
            let (mut t0, mut t1) = (u64::MAX, u64::MAX);
            let read = |query: &ID3D11Query, data: *mut std::ffi::c_void, size: usize| unsafe {
                let _ = context.GetData(query, Some(data), size as u32, 0);
            };
            let started = std::time::Instant::now();
            while info.Frequency == 0 || t0 == u64::MAX || t1 == u64::MAX {
                read(
                    &disjoint,
                    (&raw mut info).cast(),
                    std::mem::size_of_val(&info),
                );
                read(&start, (&raw mut t0).cast(), 8);
                read(&end, (&raw mut t1).cast(), 8);
                assert!(started.elapsed().as_secs() < 10, "видеокарта не ответила");
            }
            let per_pass = (t1 - t0) as f64 / info.Frequency as f64 * 1000.0 / RUNS as f64;
            eprintln!(
                "проход {}x{}: {per_pass:.4} мс на кадр (видеокарта, {RUNS} проходов{})",
                size.0,
                size.1,
                if info.Disjoint.as_bool() {
                    ", disjoint"
                } else {
                    ""
                }
            );
        }
    }
}
