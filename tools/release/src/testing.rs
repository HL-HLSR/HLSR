//! Заготовки для тестов упаковщика (и его unit-тестов): PE с ресурсом версии, ELF x86-64, PNG.
//! В выпускной код не входят, но лежат в библиотеке, чтобы их видели интеграционные тесты.

#![doc(hidden)]

fn utf16(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

/// `VS_VERSIONINFO` с одной таблицей строк: `FileVersion` и `ProductVersion`.
pub fn version_blob(product: &str) -> Vec<u8> {
    fn block(key: &str, value: &[u8], value_words: u16, kind: u16, children: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; 6];
        out.extend(utf16(key));
        out.extend([0, 0]);
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
        out.extend_from_slice(value);
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
        out.extend_from_slice(children);
        let length = out.len() as u16;
        out[0..2].copy_from_slice(&length.to_le_bytes());
        out[2..4].copy_from_slice(&value_words.to_le_bytes());
        out[4..6].copy_from_slice(&kind.to_le_bytes());
        out
    }
    fn string(key: &str, value: &str) -> Vec<u8> {
        let mut raw = utf16(value);
        raw.extend([0, 0]);
        let words = (raw.len() / 2) as u16;
        let mut out = block(key, &raw, words, 1, &[]);
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
        out
    }
    let mut strings = string("FileVersion", product);
    strings.extend(string("ProductVersion", product));
    let table = block("040904b0", &[], 0, 1, &strings);
    let info = block("StringFileInfo", &[], 0, 1, &table);
    block("VS_VERSION_INFO", &[0u8; 52], 52, 0, &info)
}

/// PE32+ с одной секцией `.rsrc`: каталог типов → имён → языков → данные.
pub fn pe_with_version(blob: Option<&[u8]>) -> Vec<u8> {
    let mut file = vec![0u8; 0x200];
    file[0..2].copy_from_slice(b"MZ");
    file[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
    file[0x40..0x44].copy_from_slice(b"PE\0\0");
    file[0x44..0x46].copy_from_slice(&0x8664u16.to_le_bytes());
    file[0x46..0x48].copy_from_slice(&1u16.to_le_bytes());
    file[0x54..0x56].copy_from_slice(&240u16.to_le_bytes());
    let optional = 0x58;
    file[optional..optional + 2].copy_from_slice(&0x20bu16.to_le_bytes());
    file[optional + 108..optional + 112].copy_from_slice(&16u32.to_le_bytes());
    let resource_dir = optional + 112 + 16;
    let section = optional + 240;
    let blob_len = blob.map_or(0, <[u8]>::len);
    let (rva, raw_pointer) = (0x1000u32, 0x200u32);
    if blob.is_some() {
        file[resource_dir..resource_dir + 4].copy_from_slice(&rva.to_le_bytes());
        file[resource_dir + 4..resource_dir + 8].copy_from_slice(&0x100u32.to_le_bytes());
    }
    file[section..section + 5].copy_from_slice(b".rsrc");
    let raw_len = 0x100 + blob_len as u32;
    file[section + 8..section + 12].copy_from_slice(&raw_len.to_le_bytes());
    file[section + 12..section + 16].copy_from_slice(&rva.to_le_bytes());
    file[section + 16..section + 20].copy_from_slice(&raw_len.to_le_bytes());
    file[section + 20..section + 24].copy_from_slice(&raw_pointer.to_le_bytes());
    // Ресурсы: корень (тип 3 — значок, тип 16 — версия), подкаталоги, лист.
    let mut rsrc = vec![0u8; 0x100];
    let dir = |rsrc: &mut Vec<u8>, at: usize, entries: &[(u32, u32)]| {
        rsrc[at + 14..at + 16].copy_from_slice(&(entries.len() as u16).to_le_bytes());
        for (index, (id, target)) in entries.iter().enumerate() {
            let slot = at + 16 + index * 8;
            rsrc[slot..slot + 4].copy_from_slice(&id.to_le_bytes());
            rsrc[slot + 4..slot + 8].copy_from_slice(&target.to_le_bytes());
        }
    };
    dir(&mut rsrc, 0, &[(3, 0x8000_0040), (16, 0x8000_0060)]);
    dir(&mut rsrc, 0x40, &[(1, 0x8000_0080)]);
    dir(&mut rsrc, 0x60, &[(1, 0x8000_0080)]);
    // Язык → лист; для значка лист не нужен (его не читают).
    dir(&mut rsrc, 0x80, &[(0x409, 0xa0)]);
    rsrc[0xa0..0xa4].copy_from_slice(&(rva + 0x100).to_le_bytes());
    rsrc[0xa4..0xa8].copy_from_slice(&(blob_len as u32).to_le_bytes());
    file.extend(rsrc);
    if let Some(blob) = blob {
        file.extend_from_slice(blob);
    }
    file
}

/// ELF64 little-endian x86-64 (40 байт: заголовок и хвост).
pub fn elf_stub(tail: &str) -> Vec<u8> {
    let mut bytes = vec![0u8; 40];
    bytes[..7].copy_from_slice(&[0x7f, b'E', b'L', b'F', 2, 1, 1]);
    bytes[18..20].copy_from_slice(&62u16.to_le_bytes());
    bytes.extend_from_slice(tail.as_bytes());
    bytes
}

/// Начало PNG и пара байт: достаточно для проверки сигнатуры.
pub fn png_stub() -> Vec<u8> {
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    bytes.extend_from_slice(b"icon");
    bytes
}

/// Рантайм type2: ELF с меткой `AI\x02` в `e_ident[8..11]`.
pub fn runtime_stub() -> Vec<u8> {
    let mut bytes = vec![0u8; 64];
    bytes[..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
    bytes[8..11].copy_from_slice(&[b'A', b'I', 2]);
    bytes
}
