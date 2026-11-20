//! Картинка и цвет без UI-фреймворка: RGBA по 8 бит на канал, строки сверху вниз. Растры
//! логики (картинки, эмодзи, значки, превью, ник, рамка) отдаются в этом виде; вид переводит
//! их в `RenderImage` GPUI (BGRA).

/// Пиксель или цвет RGBA, 8 бит на канал. Предумножена ли прозрачность — говорит источник.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const TRANSPARENT: Rgba = Rgba::new(0, 0, 0, 0);

    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Rgba {
        Rgba { r, g, b, a }
    }

    /// Непрозрачный цвет.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
        Rgba::new(r, g, b, 255)
    }
}

/// Картинка `width × height`, пиксели по строкам. Прямая альфа, если источник не пометил её
/// [`Rgba8Image::premultiplied`]: `RenderImage` GPUI ждёт прямую, вид делит такие на альфу.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rgba8Image {
    width: u32,
    height: u32,
    pixels: Vec<Rgba>,
    premultiplied: bool,
}

impl Rgba8Image {
    /// Прозрачная картинка.
    pub fn new(width: u32, height: u32) -> Rgba8Image {
        Rgba8Image {
            width,
            height,
            pixels: vec![Rgba::TRANSPARENT; width as usize * height as usize],
            premultiplied: false,
        }
    }

    /// Пометить: каналы умножены на альфу.
    pub fn premultiplied(mut self) -> Rgba8Image {
        self.premultiplied = true;
        self
    }

    pub fn is_premultiplied(&self) -> bool {
        self.premultiplied
    }

    /// Из байтов RGBA по строкам (`width × height × 4`).
    pub fn from_bytes(width: u32, height: u32, bytes: &[u8]) -> Rgba8Image {
        assert_eq!(
            bytes.len(),
            width as usize * height as usize * 4,
            "картинка {width}×{height}"
        );
        Rgba8Image {
            width,
            height,
            pixels: bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|&[r, g, b, a]| Rgba::new(r, g, b, a))
                .collect(),
            premultiplied: false,
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn as_slice(&self) -> &[Rgba] {
        &self.pixels
    }

    pub fn make_mut_slice(&mut self) -> &mut [Rgba] {
        &mut self.pixels
    }

    /// Байты RGBA по строкам.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.pixels
            .iter()
            .flat_map(|p| [p.r, p.g, p.b, p.a])
            .collect()
    }

    /// Сколько байт занимают пиксели.
    pub fn byte_len(&self) -> usize {
        self.pixels.len() * 4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_round_trip() {
        let bytes: Vec<u8> = (0..2 * 3 * 4).map(|i| i as u8).collect();
        let image = Rgba8Image::from_bytes(2, 3, &bytes);
        assert_eq!(image.as_slice()[1], Rgba::new(4, 5, 6, 7));
        assert_eq!(image.to_bytes(), bytes);
        assert_eq!(image.byte_len(), 24);
        assert_eq!(Rgba8Image::new(2, 2).as_slice(), [Rgba::TRANSPARENT; 4]);
    }
}
