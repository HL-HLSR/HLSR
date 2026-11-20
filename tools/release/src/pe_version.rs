//! `ProductVersion` исполняемого файла Windows из ресурса `VS_VERSION_INFO` (PE, секция ресурсов).
//! Апдейтер 0.4.7 читает его PowerShell-ом (`updater/core/install.js:96-150`,
//! `(Get-Item …).VersionInfo.ProductVersion`) и сверяет первые три числа с версией выпуска; тот же
//! разбор нужен упаковщику и приёмке на любой ОС, где PowerShell и `GetFileVersionInfoW` нет
//! (Linux-сборка выпуска Windows). Читаются только заголовки и сам ресурс, а не весь файл.

use crate::error::{At, Result, fail};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const RT_VERSION: u32 = 16;
const MAX_RESOURCE: u32 = 256 * 1024;

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn read_at<R: Read + Seek>(reader: &mut R, offset: u64, len: usize) -> Result<Vec<u8>> {
    let mut bytes = vec![0u8; len];
    if reader.seek(SeekFrom::Start(offset)).is_err() || reader.read_exact(&mut bytes).is_err() {
        return fail("Файл не похож на исполняемый файл Windows (PE): чтение за пределом");
    }
    Ok(bytes)
}

struct Section {
    virtual_address: u32,
    virtual_size: u32,
    raw_pointer: u32,
    raw_size: u32,
}

fn rva_to_offset(sections: &[Section], rva: u32) -> Option<u64> {
    sections.iter().find_map(|section| {
        let span = section.virtual_size.max(section.raw_size);
        let inside = rva >= section.virtual_address && rva - section.virtual_address < span;
        inside.then(|| u64::from(section.raw_pointer) + u64::from(rva - section.virtual_address))
    })
}

/// Записи каталога ресурсов: `(имя или номер, смещение; старший бит — подкаталог)`.
fn directory<R: Read + Seek>(reader: &mut R, offset: u64) -> Result<Vec<(u32, u32)>> {
    let head = read_at(reader, offset, 16)?;
    let count =
        usize::from(u16_at(&head, 12).unwrap_or(0)) + usize::from(u16_at(&head, 14).unwrap_or(0));
    if count > 4096 {
        return fail("Каталог ресурсов PE слишком велик");
    }
    let raw = read_at(reader, offset + 16, count * 8)?;
    Ok((0..count)
        .map(|index| {
            (
                u32_at(&raw, index * 8).unwrap_or(0),
                u32_at(&raw, index * 8 + 4).unwrap_or(0),
            )
        })
        .collect())
}

/// Блок `VS_VERSIONINFO` целиком (`None` — в файле нет ресурса версии).
fn version_resource<R: Read + Seek>(reader: &mut R) -> Result<Option<Vec<u8>>> {
    let dos = read_at(reader, 0, 64)?;
    if dos.get(..2) != Some(b"MZ") {
        return fail("Файл не похож на исполняемый файл Windows (PE): нет MZ");
    }
    let pe = u64::from(u32_at(&dos, 0x3c).unwrap_or(0));
    let coff = read_at(reader, pe, 24)?;
    if coff.get(..4) != Some(b"PE\0\0") {
        return fail("Файл не похож на исполняемый файл Windows (PE): нет сигнатуры PE");
    }
    let section_count = usize::from(u16_at(&coff, 6).unwrap_or(0));
    let optional_len = usize::from(u16_at(&coff, 20).unwrap_or(0));
    let optional = read_at(reader, pe + 24, optional_len)?;
    let directories = match u16_at(&optional, 0) {
        Some(0x10b) => 96,
        Some(0x20b) => 112,
        _ => return fail("Файл не похож на исполняемый файл Windows (PE): неизвестный формат"),
    };
    let sizes = u32_at(&optional, directories - 4).unwrap_or(0);
    if sizes <= 2 {
        return Ok(None);
    }
    let resource_rva = u32_at(&optional, directories + 16).unwrap_or(0);
    if resource_rva == 0 {
        return Ok(None);
    }
    let table = read_at(reader, pe + 24 + optional_len as u64, section_count * 40)?;
    let sections: Vec<Section> = (0..section_count)
        .map(|index| {
            let at = index * 40;
            Section {
                virtual_size: u32_at(&table, at + 8).unwrap_or(0),
                virtual_address: u32_at(&table, at + 12).unwrap_or(0),
                raw_size: u32_at(&table, at + 16).unwrap_or(0),
                raw_pointer: u32_at(&table, at + 20).unwrap_or(0),
            }
        })
        .collect();
    let Some(root) = rva_to_offset(&sections, resource_rva) else {
        return Ok(None);
    };
    // Тип → имя → язык: берётся первый ресурс типа RT_VERSION.
    let Some((_, mut target)) = directory(reader, root)?
        .into_iter()
        .find(|(id, _)| id & 0x8000_0000 == 0 && *id == RT_VERSION)
    else {
        return Ok(None);
    };
    for _ in 0..2 {
        if target & 0x8000_0000 == 0 {
            return Ok(None);
        }
        let Some((_, next)) = directory(reader, root + u64::from(target & 0x7fff_ffff))?
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        target = next;
    }
    if target & 0x8000_0000 != 0 {
        return Ok(None);
    }
    let entry = read_at(reader, root + u64::from(target), 16)?;
    let (data_rva, size) = (
        u32_at(&entry, 0).unwrap_or(0),
        u32_at(&entry, 4).unwrap_or(0),
    );
    if size == 0 || size > MAX_RESOURCE {
        return Ok(None);
    }
    let Some(offset) = rva_to_offset(&sections, data_rva) else {
        return Ok(None);
    };
    Ok(Some(read_at(reader, offset, size as usize)?))
}

fn utf16(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

/// Значение строки `name` из `StringFileInfo`: структура `String { wLength, wValueLength, wType,
/// szKey, Padding, Value }`, значение выровнено на 4 байта от начала ресурса.
pub fn string_value(blob: &[u8], name: &str) -> Option<String> {
    let mut key = utf16(name);
    key.extend_from_slice(&[0, 0]);
    let mut at = 6;
    while at + key.len() <= blob.len() {
        if blob[at..].starts_with(&key) && (at - 6) % 4 == 0 {
            let words = usize::from(u16_at(blob, at - 4)?);
            let start = (at + key.len()).next_multiple_of(4);
            let raw = blob.get(start..start + words * 2)?;
            let units: Vec<u16> = raw
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair))
                .take_while(|unit| *unit != 0)
                .collect();
            return String::from_utf16(&units).ok();
        }
        at += 2;
    }
    None
}

/// `ProductVersion` из открытого PE; `None` — у файла нет ресурса версии или в нём нет этой строки.
pub fn product_version_from<R: Read + Seek>(reader: &mut R) -> Result<Option<String>> {
    Ok(version_resource(reader)?.and_then(|blob| string_value(&blob, "ProductVersion")))
}

/// `ProductVersion` файла.
pub fn product_version(path: &Path) -> Result<Option<String>> {
    let mut file = File::open(path).at(path)?;
    product_version_from(&mut file)
}

/// Первые три числа `^\d+\.\d+\.\d+` после `trim` — `versionTriple` апдейтера 0.4.7, та же
/// функция, которой апдейтер сверяет `ProductVersion` установленного exe.
pub use hlsr_update::updater::install::version_triple;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{pe_with_version, version_blob};
    use std::io::Cursor;

    #[test]
    fn product_version_is_read_from_the_resource_tree() {
        let pe = pe_with_version(Some(&version_blob("0.5.0.0")));
        let version = product_version_from(&mut Cursor::new(pe)).unwrap();
        assert_eq!(version.as_deref(), Some("0.5.0.0"));
    }

    #[test]
    fn long_and_odd_lengths_keep_their_alignment() {
        for text in ["1.2.3", "10.20.30.40", "0.5.0.0-qa", "1"] {
            let pe = pe_with_version(Some(&version_blob(text)));
            let found = product_version_from(&mut Cursor::new(pe)).unwrap();
            assert_eq!(found.as_deref(), Some(text));
        }
    }

    #[test]
    fn exe_without_version_resource_is_none() {
        let pe = pe_with_version(None);
        assert_eq!(product_version_from(&mut Cursor::new(pe)).unwrap(), None);
    }

    #[test]
    fn non_pe_files_are_refused() {
        assert!(product_version_from(&mut Cursor::new(b"not an exe".to_vec())).is_err());
        let mut broken = pe_with_version(Some(&version_blob("0.5.0.0")));
        broken[0x40] = b'X';
        assert!(product_version_from(&mut Cursor::new(broken)).is_err());
    }

    #[test]
    fn version_triple_takes_the_first_three_numbers() {
        assert_eq!(version_triple("0.5.0.0").as_deref(), Some("0.5.0"));
        assert_eq!(version_triple(" 10.2.33 ").as_deref(), Some("10.2.33"));
        assert_eq!(version_triple("0.5.0-dev").as_deref(), Some("0.5.0"));
        assert_eq!(version_triple("0.5").as_deref(), None);
        assert_eq!(version_triple("a.b.c").as_deref(), None);
    }
}
