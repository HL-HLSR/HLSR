//! `nativeImage` Chromium, каким его используют `decodePng` и `resizeRgba` 0.4.7: разбор PNG и
//! JPEG (`createFromBuffer`), масштабирование `resize({quality: "good"})` и выдача пикселей
//! (`toBitmap`). Электрон считает это Skia/libpng/libjpeg-turbo; здесь то же своими руками
//! (решение 9 спецификации ПП4), сверено по снятому в Electron 44.5.1 эталону
//! (`tools/reference/online/native-image.js`).
//!
//! - Разбор PNG — как `png_set_expand` + `png_set_strip_16` libpng: палитра, серый и ключ `tRNS`
//!   становятся RGBA, 16 бит режутся до старшего байта. Гамма: при значимой `gAMA` (произведение
//!   с 2,2 дальше 0,05 от единицы) Chromium приводит цвета к sRGB ([`gamma_premul`]); `cHRM` и
//!   профили ICC не учитываются. Затем пиксели **премультиплицируются** как `SkPreMultiplyARGB`
//!   (`SkMulDiv255Round`): игре уходят премультиплицированные каналы — так делал и 0.4.7.
//! - JPEG — непрозрачный RGB (zune-jpeg вместо libjpeg-turbo: значения могут отличаться на единицы).
//! - `resize` «good» — `skia::ImageOperations::Resize` с фильтром Хэмминга (1 период): свёртка
//!   по горизонтали, потом по вертикали, коэффициенты в 14-битной дроби, промежуточный результат
//!   8 бит; вычисление фильтров — в `f32`, как у Chromium. `createFromBitmap` принимает прямые
//!   (не премультиплицированные) байты как есть, поэтому для `resizeRgba` пиксели не меняются, а
//!   после свёртки, как у Skia, альфа поднимается до наибольшего цветового канала.
//!
//! Порядок каналов — RGBA: свёртка каналы не различает, а `swapRB` 0.4.7 перестановкой лишь
//! возвращает порядок.

use std::io::Cursor;

/// Картинка в памяти: RGBA, премультиплицированная.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitmap {
    pub wide: u32,
    pub tall: u32,
    pub rgba: Vec<u8>,
}

/// `nativeImage.createFromBuffer(bytes)`: PNG, затем JPEG; пустая картинка — `None`.
pub fn create_from_buffer(bytes: &[u8]) -> Option<Bitmap> {
    decode_png(bytes).or_else(|| decode_jpeg(bytes))
}

/// `SkMulDiv255Round(a, b)`.
fn mul_div_255_round(a: u32, b: u32) -> u32 {
    let product = a * b + 128;
    (product + (product >> 8)) >> 8
}

/// `SkPreMultiplyARGB`: каналы при альфе меньше 255 умножаются на альфу.
fn premultiply(r: u8, g: u8, b: u8, a: u8) -> [u8; 4] {
    if a == 255 {
        return [r, g, b, a];
    }
    let mul = |c: u8| mul_div_255_round(u32::from(c), u32::from(a)) as u8;
    [mul(r), mul(g), mul(b), a]
}

/// Порог значимой гаммы: `gAMA`, при которой произведение с 2,2 в пределах 0,05 от единицы,
/// Chromium считает sRGB и цвета не меняет (граница снята по эталону: 0,4318…0,4772).
const SRGB_GAMMA_BAND: i64 = 5_000;

/// Показатель степени `1 / gAMA`, когда в PNG есть значимая `gAMA` (и нет `sRGB`): Chromium
/// приводит картинку из пространства с чистой степенной кривой к sRGB.
fn gamma_exponent(file_gamma: u32) -> Option<f64> {
    if file_gamma == 0 {
        return None;
    }
    let product = (f64::from(file_gamma) * 2.2).round() as i64;
    if (product - 100_000).abs() <= SRGB_GAMMA_BAND {
        return None;
    }
    Some(100_000.0 / f64::from(file_gamma))
}

/// Кривая sRGB: линейное значение 0…1 → закодированное.
fn srgb_encode(linear: f64) -> f64 {
    if linear <= 0.003_130_8 {
        12.92 * linear
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

/// Приведение уже премультиплицированного пикселя к sRGB, как `SkColorSpaceXformSteps`:
/// из премультиплицированного обратно в прямое (в `f32`-дроби), степень `exponent` и кривая sRGB,
/// снова умножение на альфу. Skia считает в `f32` с приближённой степенью, поэтому единица
/// разницы с этой формулой возможна. На непрозрачных пикселях шаги с альфой не меняют значений.
fn gamma_premul(pixel: [u8; 4], exponent: f64) -> [u8; 4] {
    let alpha = pixel[3];
    if alpha == 0 {
        return pixel;
    }
    let fix = |value: u8| {
        let straight = (f64::from(value) / f64::from(alpha)).min(1.0);
        let encoded = srgb_encode(straight.powf(exponent));
        (encoded * f64::from(alpha) + 0.5).floor().clamp(0.0, 255.0) as u8
    };
    [fix(pixel[0]), fix(pixel[1]), fix(pixel[2]), alpha]
}

fn decode_png(bytes: &[u8]) -> Option<Bitmap> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let exponent = match (reader.info().srgb, reader.info().gama_chunk) {
        // Блок sRGB задаёт пространство сам, `gAMA` рядом с ним не влияет.
        (None, Some(gamma)) => gamma_exponent(gamma.into_scaled()),
        _ => None,
    };
    let mut buffer = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buffer).ok()?;
    let (wide, tall) = (info.width, info.height);
    if wide == 0 || tall == 0 {
        return None;
    }
    let data = &buffer[..info.buffer_size()];
    let count = wide as usize * tall as usize;
    let finish = |px: [u8; 4]| match exponent {
        Some(exponent) => gamma_premul(px, exponent),
        None => px,
    };

    let mut rgba = Vec::with_capacity(count * 4);
    match info.color_type {
        png::ColorType::Grayscale => {
            for &g in data.iter().take(count) {
                rgba.extend_from_slice(&finish([g, g, g, 255]));
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for px in data.as_chunks::<2>().0.iter().take(count) {
                rgba.extend_from_slice(&finish(premultiply(px[0], px[0], px[0], px[1])));
            }
        }
        png::ColorType::Rgb => {
            for px in data.as_chunks::<3>().0.iter().take(count) {
                rgba.extend_from_slice(&finish([px[0], px[1], px[2], 255]));
            }
        }
        png::ColorType::Rgba => {
            for px in data.as_chunks::<4>().0.iter().take(count) {
                rgba.extend_from_slice(&finish(premultiply(px[0], px[1], px[2], px[3])));
            }
        }
        png::ColorType::Indexed => return None,
    }
    (rgba.len() == count * 4).then_some(Bitmap { wide, tall, rgba })
}

fn decode_jpeg(bytes: &[u8]) -> Option<Bitmap> {
    use zune_jpeg::JpegDecoder;
    use zune_jpeg::zune_core::bytestream::ZCursor;
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;

    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    let pixels = decoder.decode().ok()?;
    let info = decoder.info()?;
    let (wide, tall) = (u32::from(info.width), u32::from(info.height));
    (wide != 0 && tall != 0 && pixels.len() == wide as usize * tall as usize * 4).then_some(
        Bitmap {
            wide,
            tall,
            rgba: pixels,
        },
    )
}

const SHIFT_BITS: i32 = 14;
const FIXED_ONE: f32 = (1 << SHIFT_BITS) as f32;
/// Эпсилон `float` (`std::numeric_limits<float>::epsilon()`).
const EPSILON: f64 = 1.192_092_9e-7;

/// Окно Хэмминга в один период (`ResizeFilter::EvalHamming(1, x)`), по шагам, как их округляет
/// `float`: результат каждой операции — снова `f32`.
fn hamming1(x: f32) -> f32 {
    let x = f64::from(x);
    if x <= -1.0 || x >= 1.0 {
        return 0.0;
    }
    if x > -EPSILON && x < EPSILON {
        return 1.0;
    }
    let xpi = (x * f64::from(std::f32::consts::PI)) as f32;
    let sinc = (f64::from((f64::from(xpi).sin()) as f32) / f64::from(xpi)) as f32;
    let cos = (f64::from(xpi).cos()) as f32;
    let window = (0.54 + f64::from((0.46 * f64::from(cos)) as f32)) as f32;
    (f64::from(sinc) * f64::from(window)) as f32
}

/// Фильтр одного выходного пикселя: первый исходный пиксель и коэффициенты (дробь 14 бит).
struct Weights {
    begin: usize,
    values: Vec<i32>,
}

/// `ResizeFilter::ComputeFilters` для окна Хэмминга: по одному фильтру на выходной пиксель.
fn compute_filters(src_size: usize, dest_size: usize) -> Vec<Weights> {
    let scale = dest_size as f32 / src_size as f32;
    let inv_scale = 1.0_f32 / scale;
    let clamped_scale = scale.min(1.0);
    let support = 1.0_f32 / clamped_scale;
    let mut filters = Vec::with_capacity(dest_size);
    for i in 0..dest_size {
        let src_pixel = (i as f32 + 0.5) * inv_scale;
        let begin = (src_pixel - support).floor().max(0.0) as usize;
        let end = (((src_pixel + support).ceil()) as i64)
            .min(src_size as i64 - 1)
            .max(-1);
        let mut raw = Vec::new();
        let mut sum = 0.0_f32;
        let mut pixel = begin as i64;
        while pixel <= end {
            let distance = ((pixel as f32 + 0.5) - src_pixel) * clamped_scale;
            let value = hamming1(distance);
            raw.push(value);
            sum += value;
            pixel += 1;
        }
        let mut values: Vec<i32> = raw
            .iter()
            .map(|&value| ((value / sum) * FIXED_ONE) as i32)
            .collect();
        if !values.is_empty() {
            let leftovers = (1 << SHIFT_BITS) - values.iter().sum::<i32>();
            let middle = values.len() / 2;
            values[middle] += leftovers;
        }
        filters.push(Weights { begin, values });
    }
    filters
}

fn clamp8(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

/// `skia::ImageOperations::Resize(…, RESIZE_GOOD, …)` для RGBA 8 бит с альфой (`BGRAConvolve2D`
/// с `has_alpha`): свёртка по строкам, затем по столбцам; в конце альфа не меньше любого цвета.
pub fn resize_good(
    src: &[u8],
    wide: usize,
    tall: usize,
    out_wide: usize,
    out_tall: usize,
) -> Vec<u8> {
    let x_filters = compute_filters(wide, out_wide);
    let y_filters = compute_filters(tall, out_tall);

    let mut middle = vec![0u8; out_wide * tall * 4];
    for y in 0..tall {
        let row = &src[y * wide * 4..(y + 1) * wide * 4];
        for (x, filter) in x_filters.iter().enumerate() {
            let mut accum = [0i32; 4];
            for (k, &weight) in filter.values.iter().enumerate() {
                let at = (filter.begin + k) * 4;
                for (channel, sum) in accum.iter_mut().enumerate() {
                    *sum += weight * i32::from(row[at + channel]);
                }
            }
            let out = (y * out_wide + x) * 4;
            for channel in 0..4 {
                middle[out + channel] = clamp8(accum[channel] >> SHIFT_BITS);
            }
        }
    }

    let mut out = vec![0u8; out_wide * out_tall * 4];
    for (y, filter) in y_filters.iter().enumerate() {
        for x in 0..out_wide {
            let mut accum = [0i32; 4];
            for (k, &weight) in filter.values.iter().enumerate() {
                let at = ((filter.begin + k) * out_wide + x) * 4;
                for (channel, sum) in accum.iter_mut().enumerate() {
                    *sum += weight * i32::from(middle[at + channel]);
                }
            }
            let at = (y * out_wide + x) * 4;
            for channel in 0..4 {
                out[at + channel] = clamp8(accum[channel] >> SHIFT_BITS);
            }
            let brightest = out[at].max(out[at + 1]).max(out[at + 2]);
            if out[at + 3] < brightest {
                out[at + 3] = brightest;
            }
        }
    }
    out
}
