//! `seventv/gif.js`: свой декодер GIF (LZW, чересстрочность, прозрачность, dispose 2, не более 512
//! кадров и 1 Мпикс) и `sampleFrames` (выбор кадров для листа анимации, интервал 40…120 мс).
//! Перенос 1:1, побайтно; сверка — фикстура `gif.json` (`tools/reference/online/gif.js`).
//!
//! Что 0.4.7 делает странного, и это сохранено:
//! - слово «KwKwK» (код, которого в таблице ещё нет) собирается из слова предыдущего кода и **его
//!   последнего** знака, а по стандарту — первого; на словах, где они различаются, пиксели не те
//!   (`lzwDecode`, ветка `code >= next`). На однотонных картинках разницы нет;
//! - состояние расширения управления графикой (задержка, прозрачный индекс, dispose) сбрасывается
//!   только после нарисованного кадра: пропущенный кадр отдаёт его следующему;
//! - обрыв внутри дескриптора кадра — исключение Node (`RangeError` из `readUInt16LE`): тут
//!   [`GifRangeError`], текст которого — текст Node.
//!
//! Отступление: `minCodeSize` больше 11 (по стандарту — до 8) в 0.4.7 даёт мусор, зависящий от
//! сдвигов `int32` и выхода за массивы; здесь такой кадр пропускается.

use hlsr_core::js::math_round;

/// Не больше стольких кадров разбирается.
pub const MAX_FRAMES_DECODED: usize = 512;
/// Холст не больше мегапикселя.
pub const MAX_PIXELS: u64 = 1024 * 1024;

const MAX_CODE: usize = 4096;
/// Больше стольких индексов подряд не набирается: у 0.4.7 `new Uint8Array(fw * fh)` на кадр до
/// 4 миллиардов байт, здесь потолок — защита памяти (настоящие кадры на порядки меньше).
const MAX_INDICES: u64 = 1 << 26;
const MIN_INTERVAL_MS: f64 = 40.0;
const MAX_INTERVAL_MS: f64 = 120.0;

/// Кадр: полный холст RGBA (прямая альфа, 0 или 255) и задержка.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GifFrame {
    pub rgba: Vec<u8>,
    pub delay_ms: u32,
}

/// Разобранный GIF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedGif {
    pub wide: u32,
    pub tall: u32,
    pub frames: Vec<GifFrame>,
}

/// `RangeError` Node: `buf.readUInt16LE(offset)` за концом буфера.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("The value of \"offset\" is out of range. It must be >= 0 and <= {max}. Received {offset}")]
pub struct GifRangeError {
    pub offset: usize,
    pub max: usize,
}

fn u16_at(buf: &[u8], offset: usize) -> Result<u32, GifRangeError> {
    match buf.get(offset..offset + 2) {
        Some(bytes) => Ok(u32::from(bytes[0]) | (u32::from(bytes[1]) << 8)),
        None => Err(GifRangeError {
            offset,
            max: buf.len().saturating_sub(2),
        }),
    }
}

/// Результат LZW: индексы палитры по порядку; за концом — нули (`new Uint8Array(pixelCount)`).
type Indices = Vec<u8>;

/// `lzwDecode(data, minCodeSize, pixelCount)`: `None` — как `null` в 0.4.7.
fn lzw_decode(data: &[u8], min_code_size: u32, pixel_count: u64) -> Option<Indices> {
    let pixel_count = pixel_count.min(MAX_INDICES);
    let clear_code: i32 = 1 << min_code_size;
    let end_code = clear_code + 1;
    let mut code_size = min_code_size as i32 + 1;
    let mut code_mask: i32 = (1 << code_size) - 1;
    let mut next = end_code + 1;

    let mut prefix = vec![0i32; MAX_CODE + 2];
    let mut suffix = vec![0u8; MAX_CODE + 2];
    let mut stack = vec![0u8; MAX_CODE + 2];
    for i in 0..clear_code as usize {
        prefix[i] = 0;
        suffix[i] = i as u8;
    }

    let mut out: Indices = Vec::new();
    let mut bit_buffer: i32 = 0;
    let mut bit_count: i32 = 0;
    let mut data_pos = 0usize;
    let mut previous: i32 = -1;
    let produced = |out: Indices| if out.is_empty() { None } else { Some(out) };

    while (out.len() as u64) < pixel_count {
        while bit_count < code_size {
            if data_pos >= data.len() {
                return produced(out);
            }
            bit_buffer |= i32::from(data[data_pos]) << bit_count;
            data_pos += 1;
            bit_count += 8;
        }

        let code = bit_buffer & code_mask;
        bit_buffer >>= code_size;
        bit_count -= code_size;

        if code == clear_code {
            code_size = min_code_size as i32 + 1;
            code_mask = (1 << code_size) - 1;
            next = end_code + 1;
            previous = -1;
            continue;
        }
        if code == end_code {
            break;
        }

        let mut current = code;
        let mut stack_pos = 0usize;

        if code >= next {
            if previous < 0 {
                return produced(out);
            }
            stack[stack_pos] = suffix[previous as usize];
            stack_pos += 1;
            current = previous;
        }

        let mut guard = 0usize;
        while current >= clear_code {
            guard += 1;
            if guard > MAX_CODE {
                return produced(out);
            }
            stack[stack_pos] = suffix[current as usize];
            stack_pos += 1;
            current = prefix[current as usize];
        }
        stack[stack_pos] = suffix[current as usize];
        stack_pos += 1;

        while stack_pos > 0 && (out.len() as u64) < pixel_count {
            stack_pos -= 1;
            out.push(stack[stack_pos]);
        }

        if previous >= 0 && (next as usize) < MAX_CODE {
            prefix[next as usize] = previous;
            suffix[next as usize] = current as u8;
            next += 1;
            if (next & code_mask) == 0 && (next as usize) < MAX_CODE {
                code_size += 1;
                code_mask = (1 << code_size) - 1;
            }
        }
        previous = code;
    }

    Some(out)
}

/// `readSubBlocks(buf, pos)`: данные подблоков и позиция за ними; `None` в данных — подблок
/// длиннее файла.
fn read_sub_blocks(buf: &[u8], mut pos: usize) -> (Option<Vec<u8>>, usize) {
    let mut data = Vec::new();
    while pos < buf.len() {
        let n = usize::from(buf[pos]);
        if n == 0 {
            pos += 1;
            break;
        }
        pos += 1;
        if pos + n > buf.len() {
            return (None, buf.len());
        }
        data.extend_from_slice(&buf[pos..pos + n]);
        pos += n;
    }
    (Some(data), pos)
}

/// `skipSubBlocks(buf, pos)`.
fn skip_sub_blocks(buf: &[u8], mut pos: usize) -> usize {
    while pos < buf.len() && buf[pos] != 0 {
        pos += usize::from(buf[pos]) + 1;
    }
    pos + 1
}

/// `readPalette(buf, pos, size)`: `size` цветов, за концом файла — нули.
fn read_palette(buf: &[u8], pos: usize, size: usize) -> Vec<u8> {
    (0..size * 3)
        .map(|i| buf.get(pos + i).copied().unwrap_or(0))
        .collect()
}

/// `deinterlaceRow(row, height)`: строка кадра по номеру в потоке.
fn deinterlace_row(row: i64, height: i64) -> i64 {
    let ceil_div = |a: i64, b: i64| (a as f64 / b as f64).ceil() as i64;
    let p1 = ceil_div(height, 8);
    let p2 = p1 + ceil_div(height - 4, 8);
    let p3 = p2 + ceil_div(height - 2, 4);
    if row < p1 {
        row * 8
    } else if row < p2 {
        (row - p1) * 8 + 4
    } else if row < p3 {
        (row - p2) * 4 + 2
    } else {
        (row - p3) * 2 + 1
    }
}

/// `decodeGif(buf)`: `Ok(None)` — не GIF, пустой или слишком большой холст, нет кадров;
/// `Err` — исключение Node на обрезанном дескрипторе кадра.
pub fn decode_gif(buf: &[u8]) -> Result<Option<DecodedGif>, GifRangeError> {
    if buf.len() < 13 || &buf[0..3] != b"GIF" {
        return Ok(None);
    }
    let wide = u16_at(buf, 6)?;
    let tall = u16_at(buf, 8)?;
    if wide == 0 || tall == 0 || u64::from(wide) * u64::from(tall) > MAX_PIXELS {
        return Ok(None);
    }
    let (wide_px, tall_px) = (wide as usize, tall as usize);

    let packed = buf[10];
    let mut pos = 13usize;
    let mut global_palette: Option<Vec<u8>> = None;
    if packed & 0x80 != 0 {
        let size = 2usize << (packed & 7);
        global_palette = Some(read_palette(buf, pos, size));
        pos += size * 3;
    }

    let mut canvas = vec![0u8; wide_px * tall_px * 4];
    let mut frames: Vec<GifFrame> = Vec::new();
    let mut delay_ms: u32 = 0;
    let mut transparent_index: i32 = -1;
    let mut disposal: u8 = 0;

    while pos < buf.len() && frames.len() < MAX_FRAMES_DECODED {
        let block = buf[pos];
        if block == 0x3b {
            break;
        }

        if block == 0x21 {
            let label = buf.get(pos + 1).copied();
            pos += 2;
            if label == Some(0xf9) && pos + 5 <= buf.len() {
                let flags = buf[pos + 1];
                disposal = (flags >> 2) & 7;
                transparent_index = if flags & 1 != 0 {
                    i32::from(buf[pos + 4])
                } else {
                    -1
                };
                delay_ms = u16_at(buf, pos + 2)? * 10;
            }
            pos = skip_sub_blocks(buf, pos);
            continue;
        }

        if block != 0x2c {
            pos += 1;
            continue;
        }

        let fx = u16_at(buf, pos + 1)? as i64;
        let fy = u16_at(buf, pos + 3)? as i64;
        let fw = u16_at(buf, pos + 5)? as i64;
        let fh = u16_at(buf, pos + 7)? as i64;
        let lflags = buf.get(pos + 9).copied().unwrap_or(0);
        pos += 10;

        let mut palette = global_palette.clone();
        if lflags & 0x80 != 0 {
            let size = 2usize << (lflags & 7);
            palette = Some(read_palette(buf, pos, size));
            pos += size * 3;
        }
        let interlaced = lflags & 0x40 != 0;

        let min_code_size = buf.get(pos).copied();
        pos += 1;
        let (sub_data, next_pos) = read_sub_blocks(buf, pos);
        pos = next_pos;

        let (Some(palette), Some(sub_data)) = (palette, sub_data) else {
            continue;
        };
        if fw <= 0 || fh <= 0 {
            continue;
        }

        let pixel_count = fw as u64 * fh as u64;
        let indices = match min_code_size {
            // Нет байта minCodeSize (файл кончился): 0.4.7 крутит декодер с NaN и получает нули.
            None => Some(Vec::new()),
            Some(size) if size > 11 => None,
            Some(size) => lzw_decode(&sub_data, u32::from(size), pixel_count),
        };
        let Some(indices) = indices else {
            continue;
        };

        let visible_cols = if fx < i64::from(wide) {
            fw.min(i64::from(wide) - fx)
        } else {
            0
        };
        for row in 0..fh {
            let src_row = if interlaced {
                deinterlace_row(row, fh)
            } else {
                row
            };
            let y = fy + src_row;
            if y < 0 || y >= i64::from(tall) {
                continue;
            }
            for col in 0..visible_cols {
                let x = fx + col;
                let at = (row as u64 * fw as u64 + col as u64) as usize;
                let index = indices.get(at).copied().unwrap_or(0);
                if i32::from(index) == transparent_index {
                    continue;
                }
                let p = (y as usize * wide_px + x as usize) * 4;
                let q = usize::from(index) * 3;
                canvas[p] = palette.get(q).copied().unwrap_or(0);
                canvas[p + 1] = palette.get(q + 1).copied().unwrap_or(0);
                canvas[p + 2] = palette.get(q + 2).copied().unwrap_or(0);
                canvas[p + 3] = 255;
            }
        }

        frames.push(GifFrame {
            rgba: canvas.clone(),
            delay_ms: if delay_ms > 0 { delay_ms } else { 100 },
        });

        if disposal == 2 {
            for row in 0..fh {
                let y = fy + row;
                if y < 0 || y >= i64::from(tall) {
                    continue;
                }
                let count = fw.min(i64::from(wide) - fx) * 4;
                if count > 0 {
                    let from = (y as usize * wide_px + fx.max(0) as usize) * 4;
                    canvas[from..from + count as usize].fill(0);
                }
            }
        }

        delay_ms = 0;
        transparent_index = -1;
        disposal = 0;
    }

    if frames.is_empty() {
        return Ok(None);
    }
    Ok(Some(DecodedGif { wide, tall, frames }))
}

/// Выбранные кадры: номера в `decoded.frames` и интервал между ними.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampledFrames {
    pub picks: Vec<usize>,
    pub interval_ms: u32,
}

/// `clampInterval(ms)`: 40…120 мс, нерасчётное — 100.
fn clamp_interval(ms: f64) -> u32 {
    if ms.is_nan() || ms <= 0.0 {
        return 100;
    }
    math_round(ms).clamp(MIN_INTERVAL_MS, MAX_INTERVAL_MS) as u32
}

/// `sampleFrames(decoded, maxFrames)`.
pub fn sample_frames(decoded: &DecodedGif, max_frames: usize) -> SampledFrames {
    let src = &decoded.frames;
    if src.len() <= max_frames {
        let total: f64 = src.iter().map(|frame| f64::from(frame.delay_ms)).sum();
        return SampledFrames {
            picks: (0..src.len()).collect(),
            interval_ms: clamp_interval(total / src.len() as f64),
        };
    }

    let total_ms: f64 = src.iter().map(|frame| f64::from(frame.delay_ms)).sum();
    let step = total_ms / max_frames as f64;

    let mut picked: Vec<usize> = Vec::new();
    let mut cursor = 0usize;
    let mut elapsed = 0.0_f64;
    let mut want = step / 2.0;

    let mut i = 0usize;
    while i < src.len() && picked.len() < max_frames {
        elapsed += f64::from(src[i].delay_ms);
        while picked.len() < max_frames && want <= elapsed {
            picked.push(i);
            want += step;
        }
        cursor = i;
        i += 1;
    }
    while picked.len() < max_frames {
        picked.push(cursor);
    }

    SampledFrames {
        picks: picked,
        interval_ms: clamp_interval(step),
    }
}
