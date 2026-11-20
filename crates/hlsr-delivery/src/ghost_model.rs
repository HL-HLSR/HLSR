//! Геометрия модели призрака Ghosting (`gh_model`) для предпросмотра в «Персонализации»
//! (`ghost-model.js` 0.4.9). Модель Source — три файла: `.mdl` (части тела, модели, сетки),
//! `.vvd` (вершины в позе привязки, координаты модели), `.dx90.vtx` (треугольники по вершинам
//! сеток). Текстуры не нужны: игра красит призрака цветом (`kRenderTransColor`), предпросмотр
//! делает так же.
//!
//! Где ищется файл — как его найдёт игра: `ghostingmod/custom/*` (по алфавиту), сам мод, затем
//! VPK (`*_dir.vpk` мода и hl2). Читаются только пути `models/….mdl` внутри этих мест.
//!
//! Выход за буфер при разборе — ошибка с кодом `ERR_OUT_OF_RANGE`, как у `Buffer.read*` Node.

use crate::cfg_files::real_path;
use hlsr_core::js;
use hlsr_core::js::errno;
use hlsr_core::js::path::NodePath;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
/// Дерево каталога VPK (у `hl2_misc_dir.vpk` — меньше мегабайта); данные архива не читаются.
const MAX_VPK_TREE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_TRIANGLES: usize = 60000;
/// `mstudiovertex_t`.
const VVD_VERTEX_SIZE: i64 = 48;
const HL2_VPKS: [&str; 6] = [
    "hl2_english",
    "hl2_pak",
    "hl2_textures",
    "hl2_sound_vo_english",
    "hl2_sound_misc",
    "hl2_misc",
];

/// `GhostModelError`: код (`ASSET_*`, `MODEL_*`, `VPK_INVALID`, код ошибки fs) и текст.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct GhostModelError {
    pub code: String,
    pub message: String,
}

fn fail<T>(code: &str, message: &str) -> Result<T, GhostModelError> {
    Err(GhostModelError {
        code: code.to_owned(),
        message: message.to_owned(),
    })
}

fn io_error(error: &std::io::Error) -> GhostModelError {
    GhostModelError {
        code: errno::code(error).map_or_else(|| "ASSET_READ_FAILED".to_owned(), |c| c.into_owned()),
        message: error.to_string(),
    }
}

/// Геометрия: `x, y, z` по три вершины на треугольник, координаты модели.
#[derive(Debug, Clone, PartialEq)]
pub struct Mesh {
    pub positions: Vec<f64>,
    pub triangles: usize,
}

/// `/^models\/[A-Za-z0-9_\-./ ]+\.mdl$/i`.
fn is_model_value(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() > 11
        && bytes[..7].eq_ignore_ascii_case(b"models/")
        && bytes[bytes.len() - 4..].eq_ignore_ascii_case(b".mdl")
        && bytes[7..bytes.len() - 4]
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'/' | b' '))
}

/// `normalizeValue(value)`: значение `gh_model` — путь внутри мода, без `..` и абсолютных путей.
pub fn normalize_value(value: Option<&str>) -> Result<String, GhostModelError> {
    let text = value.map_or_else(String::new, |value| js::trim(value).replace('\\', "/"));
    if text.is_empty()
        || text.encode_utf16().count() > 200
        || !is_model_value(&text)
        || text.split('/').any(|part| part == ".." || part == ".")
    {
        return fail("ASSET_INVALID_PATH", "Invalid model path");
    }
    Ok(text)
}

// ---- чтение буферов как у Node

fn out_of_range() -> GhostModelError {
    GhostModelError {
        code: "ERR_OUT_OF_RANGE".to_owned(),
        message: "The value of \"offset\" is out of range.".to_owned(),
    }
}

fn bytes_at(buf: &[u8], offset: i64, len: usize) -> Result<&[u8], GhostModelError> {
    let start = usize::try_from(offset).map_err(|_| out_of_range())?;
    buf.get(start..start.checked_add(len).ok_or_else(out_of_range)?)
        .ok_or_else(out_of_range)
}

fn i32_at(buf: &[u8], offset: i64) -> Result<i64, GhostModelError> {
    let b = bytes_at(buf, offset, 4)?;
    Ok(i64::from(i32::from_le_bytes([b[0], b[1], b[2], b[3]])))
}

fn u32_at(buf: &[u8], offset: i64) -> Result<i64, GhostModelError> {
    let b = bytes_at(buf, offset, 4)?;
    Ok(i64::from(u32::from_le_bytes([b[0], b[1], b[2], b[3]])))
}

fn u16_at(buf: &[u8], offset: i64) -> Result<i64, GhostModelError> {
    let b = bytes_at(buf, offset, 2)?;
    Ok(i64::from(u16::from_le_bytes([b[0], b[1]])))
}

fn u8_at(buf: &[u8], offset: i64) -> Result<i64, GhostModelError> {
    Ok(i64::from(bytes_at(buf, offset, 1)?[0]))
}

fn f32_at(buf: &[u8], offset: i64) -> Result<f64, GhostModelError> {
    let b = bytes_at(buf, offset, 4)?;
    Ok(f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]])))
}

/// `buf.toString("latin1", 0, 4) === magic`.
fn has_magic(buf: &[u8], magic: &[u8; 4]) -> bool {
    buf.get(..4) == Some(magic)
}

/// `fs.readSync(fd, Buffer.alloc(len), 0, len, at)`: недочитанное остаётся нулями.
fn read_at(file: &Path, at: u64, len: usize) -> Result<Vec<u8>, GhostModelError> {
    let mut buf = Vec::new();
    read_into(file, at, len, &mut buf)?;
    Ok(buf)
}

/// [`read_at`] в конец `out`: части записи VPK ложатся в один буфер, без лишней копии.
fn read_into(file: &Path, at: u64, len: usize, out: &mut Vec<u8>) -> Result<(), GhostModelError> {
    let mut handle = fs::File::open(file).map_err(|e| io_error(&e))?;
    let start = out.len();
    out.resize(start + len, 0);
    handle.seek(SeekFrom::Start(at)).map_err(|e| io_error(&e))?;
    let mut filled = start;
    while filled < out.len() {
        match handle.read(&mut out[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) => return Err(io_error(&error)),
        }
    }
    Ok(())
}

// ---- VPK (v1 и v2): каталог файлов один раз на файл, данные — по запросу.

#[derive(Debug, Clone)]
struct VpkEntry {
    /// Один путь на архив: записей в каталоге VPK hl2 — десятки тысяч.
    file: Arc<Path>,
    archive: i64,
    offset: i64,
    length: i64,
    preload_at: i64,
    preload: i64,
    data_at: i64,
}

type VpkDir = Arc<HashMap<String, VpkEntry>>;

struct Cached {
    mtime: Option<SystemTime>,
    entries: Result<VpkDir, GhostModelError>,
}

/// Сломанный VPK запоминается тоже, чтобы не разбирать его на каждый запрос.
static VPK_CACHE: LazyLock<Mutex<HashMap<PathBuf, Cached>>> = LazyLock::new(Mutex::default);

/// `readVpkDir(file)`: дерево каталога VPK (ключи — пути в нижнем регистре).
fn read_vpk_dir(file: &Path) -> Result<VpkDir, GhostModelError> {
    let mtime = fs::metadata(file)
        .map_err(|e| io_error(&e))?
        .modified()
        .ok();
    let cache = || VPK_CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(cached) = cache().get(file).filter(|cached| cached.mtime == mtime) {
        return cached.entries.clone();
    }
    // Разбор (чтение дерева с диска) — без блокировки: другие предпросмотры не ждут.
    let entries = parse_vpk_dir(file).map(Arc::new);
    cache().insert(
        file.to_path_buf(),
        Cached {
            mtime,
            entries: entries.clone(),
        },
    );
    entries
}

/// Только для самотестов: разбор каталога VPK (с кэшем).
pub fn vpk_entry_count(file: &Path) -> Result<usize, GhostModelError> {
    read_vpk_dir(file).map(|entries| entries.len())
}

/// Читаются только заголовок и дерево: у одиночного VPK (`ghostingmod_pak.vpk`) за деревом —
/// все данные архива.
fn parse_vpk_dir(file: &Path) -> Result<HashMap<String, VpkEntry>, GhostModelError> {
    let head = read_at(file, 0, 28)?;
    if u32_at(&head, 0)? != 0x55aa_1234 {
        return fail("VPK_INVALID", "Not a VPK");
    }
    let tree_size = u32_at(&head, 8)?;
    let header: i64 = if u32_at(&head, 4)? == 2 { 28 } else { 12 };
    if tree_size as u64 > MAX_VPK_TREE_BYTES {
        return fail("VPK_INVALID", "VPK tree too large");
    }
    let buf = read_at(file, 0, (header + tree_size) as usize)?;
    let broken = || GhostModelError {
        code: "VPK_INVALID".to_owned(),
        message: "Broken VPK tree".to_owned(),
    };
    let shared: Arc<Path> = Arc::from(file);
    let mut p = header as usize;
    // Без нуля в конце разбор пошёл бы сначала.
    let text = |p: &mut usize| -> Result<String, GhostModelError> {
        let end = buf
            .get(*p..)
            .and_then(|rest| rest.iter().position(|b| *b == 0))
            .ok_or_else(broken)?;
        let s: String = buf[*p..*p + end].iter().map(|b| char::from(*b)).collect();
        *p += end + 1;
        Ok(s)
    };
    let mut entries = HashMap::new();
    loop {
        let ext = text(&mut p)?;
        if ext.is_empty() {
            break;
        }
        loop {
            let dir = text(&mut p)?;
            if dir.is_empty() {
                break;
            }
            loop {
                let name = text(&mut p)?;
                if name.is_empty() {
                    break;
                }
                let at = p as i64;
                let preload = u16_at(&buf, at + 4).map_err(|_| broken())?;
                let archive = u16_at(&buf, at + 6).map_err(|_| broken())?;
                let offset = u32_at(&buf, at + 8).map_err(|_| broken())?;
                let length = u32_at(&buf, at + 12).map_err(|_| broken())?;
                p += 18;
                if p as i64 + preload > buf.len() as i64 {
                    return Err(broken());
                }
                let prefix = if dir == " " {
                    String::new()
                } else {
                    format!("{dir}/")
                };
                let full = format!("{prefix}{name}.{ext}");
                entries.insert(
                    full.to_lowercase(),
                    VpkEntry {
                        file: shared.clone(),
                        archive,
                        offset,
                        length,
                        preload_at: p as i64,
                        preload,
                        data_at: header + tree_size,
                    },
                );
                p += preload as usize;
            }
        }
    }
    Ok(entries)
}

fn read_vpk_entry(entry: &VpkEntry) -> Result<Vec<u8>, GhostModelError> {
    if (entry.preload + entry.length) as u64 > MAX_FILE_BYTES {
        return fail("ASSET_TOO_LARGE", "Model file too large");
    }
    let mut out = Vec::with_capacity((entry.preload + entry.length) as usize);
    if entry.preload > 0 {
        read_into(
            &entry.file,
            entry.preload_at as u64,
            entry.preload as usize,
            &mut out,
        )?;
    }
    if entry.length > 0 {
        let (archive, at) = if entry.archive == 0x7fff {
            (entry.file.to_path_buf(), entry.data_at + entry.offset)
        } else {
            let name = entry.file.to_string_lossy();
            let lower = name.to_ascii_lowercase();
            let archive = match lower.strip_suffix("_dir.vpk") {
                Some(stem) => format!("{}_{:03}.vpk", &name[..stem.len()], entry.archive),
                None => name.into_owned(),
            };
            (PathBuf::from(archive), entry.offset)
        };
        read_into(&archive, at as u64, entry.length as usize, &mut out)?;
    }
    Ok(out)
}

// ---- места, где игра ищет файл модели

enum Source {
    Dir(PathBuf),
    Vpk(PathBuf),
}

/// «name.vpk» на диске — `name_dir.vpk`.
fn vpk_of(base: &Path) -> Option<Source> {
    ["_dir.vpk", ".vpk"].iter().find_map(|suffix| {
        let mut name = base.as_os_str().to_owned();
        name.push(suffix);
        let file = PathBuf::from(name);
        file.exists().then_some(Source::Vpk(file))
    })
}

/// Папки и VPK внутри `dir` по алфавиту. Один битый элемент (ссылка в никуда, занятый файл)
/// не прячет остальные. Архив из нескольких частей монтируется по `_dir.vpk`, части `_000…`
/// не отдельно.
fn each(dir: &Path) -> Vec<Source> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .collect();
    names.sort();
    names
        .into_iter()
        .filter_map(|name| {
            let full = dir.join(&name);
            let meta = fs::metadata(&full).ok()?;
            if meta.is_dir() {
                return Some(Source::Dir(full));
            }
            let lower = name.to_ascii_lowercase();
            let part = lower.len() >= 8
                && lower.ends_with(".vpk")
                && lower.as_bytes()[lower.len() - 8] == b'_'
                && lower.as_bytes()[lower.len() - 7..lower.len() - 4]
                    .iter()
                    .all(u8::is_ascii_digit);
            (lower.ends_with(".vpk") && !part).then_some(Source::Vpk(full))
        })
        .collect()
}

/// Настоящий путь (ссылки раскрыты); `None` — файла нет.
fn real(file: &Path) -> Option<String> {
    real_path(file)
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
}

fn inside(child: Option<&str>, parent: Option<&str>) -> bool {
    let (Some(child), Some(parent)) = (child, parent) else {
        return false;
    };
    let sep = NodePath::native().sep();
    child
        .to_lowercase()
        .starts_with(&format!("{}{sep}", parent.to_lowercase()))
}

/// Места по порядку `SearchPaths` из `ghostingmod/gameinfo.txt`: custom/* мода и hl2 (папки и
/// VPK по алфавиту), файлы мода, его VPK, content/*, mapbase_shared, VPK hl2 по списку, файлы hl2.
fn sources(install: &Path) -> Vec<Source> {
    let r#mod = install.join("ghostingmod");
    let hl2 = install.join("hl2");
    let shared = install.join("mapbase_shared");
    let mut out = each(&r#mod.join("custom"));
    out.extend(each(&hl2.join("custom")));
    out.push(Source::Dir(r#mod.clone()));
    out.extend(vpk_of(&r#mod.join("ghostingmod_pak")));
    out.extend(each(&r#mod.join("content")));
    out.push(Source::Dir(shared.join("shared_misc")));
    out.extend(vpk_of(&shared.join("shared_content")));
    out.extend(HL2_VPKS.iter().filter_map(|name| vpk_of(&hl2.join(name))));
    out.push(Source::Dir(hl2));
    out
}

fn read_game_file(install: &Path, relative: &str) -> Result<Option<Vec<u8>>, GhostModelError> {
    let root = real(install);
    for source in sources(install) {
        match source {
            Source::Dir(dir) => {
                let file = relative.split('/').fold(dir, |path, part| path.join(part));
                // Ссылка внутри мода на файл снаружи игры не читается.
                if !inside(real(&file).as_deref(), root.as_deref()) {
                    continue;
                }
                let Ok(meta) = fs::metadata(&file) else {
                    continue;
                };
                if !meta.is_file() {
                    continue;
                }
                if meta.len() > MAX_FILE_BYTES {
                    return fail("ASSET_TOO_LARGE", "Model file too large");
                }
                return fs::read(&file).map(Some).map_err(|e| io_error(&e));
            }
            Source::Vpk(vpk) => {
                if !inside(real(&vpk).as_deref(), root.as_deref()) {
                    continue;
                }
                let Ok(entries) = read_vpk_dir(&vpk) else {
                    continue;
                };
                if let Some(entry) = entries.get(&relative.to_lowercase()) {
                    return read_vpk_entry(entry).map(Some);
                }
            }
        }
    }
    Ok(None)
}

// ---- разбор: треугольники LOD 0, первая модель каждой части тела

type Vertex = [f64; 3];

fn vvd_vertices(vvd: &[u8]) -> Result<Vec<Vertex>, GhostModelError> {
    if !has_magic(vvd, b"IDSV") {
        return fail("MODEL_INVALID", "Not a VVD");
    }
    let num_lods = i32_at(vvd, 12)?;
    let lod0 = i32_at(vvd, 16)?;
    let num_fixups = i32_at(vvd, 48)?;
    let fixup_start = i32_at(vvd, 52)?;
    let data_start = i32_at(vvd, 56)?;
    let len = vvd.len() as i64;
    if num_lods < 1 || lod0 < 0 || data_start + lod0 * VVD_VERTEX_SIZE > len {
        return fail("MODEL_INVALID", "Bad VVD");
    }
    // Вершин не больше, чем помещается в сам файл: счётчики из файла не раздувают память.
    let stored = (len - data_start).div_euclid(VVD_VERTEX_SIZE);
    if lod0 > stored {
        return fail("MODEL_INVALID", "Bad VVD");
    }
    let at = |i: i64| -> Result<Vertex, GhostModelError> {
        let o = data_start + i * VVD_VERTEX_SIZE + 16;
        Ok([f32_at(vvd, o)?, f32_at(vvd, o + 4)?, f32_at(vvd, o + 8)?])
    };
    if num_fixups == 0 {
        return (0..lod0).map(at).collect();
    }
    if num_fixups < 0 || fixup_start < 0 || fixup_start + num_fixups * 12 > len {
        return fail("MODEL_INVALID", "Bad VVD fixups");
    }
    // Вершины LOD 0 собираются из таблицы fixup (так же делает движок); итог — не больше lod0.
    let mut out = Vec::new();
    for f in 0..num_fixups {
        let o = fixup_start + f * 12;
        let from = i32_at(vvd, o + 4)?;
        let count = i32_at(vvd, o + 8)?;
        if from < 0 || count < 0 || from + count > stored || out.len() as i64 + count > lod0 {
            return fail("MODEL_INVALID", "Bad VVD fixups");
        }
        for i in 0..count {
            out.push(at(from + i)?);
        }
    }
    Ok(out)
}

/// `parseMesh(mdl, vvd, vtx)`: треугольники (не больше [`MAX_TRIANGLES`]).
pub fn parse_mesh(mdl: &[u8], vvd: &[u8], vtx: &[u8]) -> Result<Mesh, GhostModelError> {
    if !has_magic(mdl, b"IDST") {
        return fail("MODEL_INVALID", "Not an MDL");
    }
    let version = i32_at(mdl, 4)?;
    if !(44..=49).contains(&version) {
        return fail("MODEL_UNSUPPORTED", &format!("MDL version {version}"));
    }
    if i32_at(vtx, 0)? != 7 {
        return fail("MODEL_UNSUPPORTED", "VTX version");
    }
    let verts = vvd_vertices(vvd)?;
    let num_body_parts = i32_at(mdl, 232)?;
    let body_part_index = i32_at(mdl, 236)?;
    let vtx_body_parts = i32_at(vtx, 28)?;
    let vtx_body_part_at = i32_at(vtx, 32)?;
    let mut positions = Vec::new();
    let mut triangles = 0usize;
    for b in 0..num_body_parts.min(vtx_body_parts) {
        let bp = body_part_index + b * 16;
        if i32_at(mdl, bp + 4)? < 1 {
            continue;
        }
        let model = bp + i32_at(mdl, bp + 12)?; // первая модель части тела
        let num_meshes = i32_at(mdl, model + 72)?;
        let mesh_index = i32_at(mdl, model + 76)?;
        let vertex_base = i32_at(mdl, model + 84)?.div_euclid(VVD_VERTEX_SIZE);
        let vbp = vtx_body_part_at + b * 8;
        if i32_at(vtx, vbp)? < 1 {
            continue;
        }
        let vmodel = vbp + i32_at(vtx, vbp + 4)?;
        if i32_at(vtx, vmodel)? < 1 {
            continue;
        }
        let lod = vmodel + i32_at(vtx, vmodel + 4)?;
        let vtx_meshes = i32_at(vtx, lod)?;
        let mesh_at = lod + i32_at(vtx, lod + 4)?;
        for m in 0..num_meshes.min(vtx_meshes) {
            let mesh = model + mesh_index + m * 116;
            let vertex_offset = i32_at(mdl, mesh + 12)?;
            let vm = mesh_at + m * 9;
            let groups = i32_at(vtx, vm)?;
            let group_at = vm + i32_at(vtx, vm + 4)?;
            for g in 0..groups {
                let sg = group_at + g * 25;
                let num_verts = i32_at(vtx, sg)?;
                let vert_at = sg + i32_at(vtx, sg + 4)?;
                let index_at = sg + i32_at(vtx, sg + 12)?;
                let num_strips = i32_at(vtx, sg + 16)?;
                let strip_at = sg + i32_at(vtx, sg + 20)?;
                let vertex_of = |i: i64| -> Result<Option<&Vertex>, GhostModelError> {
                    if i < 0 || i >= num_verts {
                        return Ok(None);
                    }
                    let id = u16_at(vtx, vert_at + i * 9 + 4)?;
                    Ok(usize::try_from(vertex_base + vertex_offset + id)
                        .ok()
                        .and_then(|at| verts.get(at)))
                };
                let mut push = |a: i64, b: i64, c: i64| -> Result<(), GhostModelError> {
                    let (va, vb, vc) = (vertex_of(a)?, vertex_of(b)?, vertex_of(c)?);
                    if let (Some(va), Some(vb), Some(vc)) = (va, vb, vc)
                        && triangles < MAX_TRIANGLES
                    {
                        positions.extend(va.iter().chain(vb).chain(vc));
                        triangles += 1;
                    }
                    Ok(())
                };
                for s in 0..num_strips {
                    let st = strip_at + s * 27;
                    let count = i32_at(vtx, st)?;
                    let first = i32_at(vtx, st + 4)?;
                    let flags = u8_at(vtx, st + 18)?;
                    let index = |k: i64| u16_at(vtx, index_at + (first + k) * 2);
                    if flags & 2 != 0 {
                        let mut k = 0;
                        while k + 2 < count {
                            if k % 2 != 0 {
                                push(index(k + 1)?, index(k)?, index(k + 2)?)?;
                            } else {
                                push(index(k)?, index(k + 1)?, index(k + 2)?)?;
                            }
                            k += 1;
                        }
                    } else {
                        let mut k = 0;
                        while k + 2 < count {
                            push(index(k)?, index(k + 1)?, index(k + 2)?)?;
                            k += 3;
                        }
                    }
                }
            }
        }
    }
    if triangles == 0 {
        return fail("MODEL_EMPTY", "Model has no triangles");
    }
    Ok(Mesh {
        positions,
        triangles,
    })
}

/// `readGhostMesh(installPath, value)`: геометрия `gh_model`, координаты округлены до сотых
/// (размер ответа меньше, точность — сотая единицы).
pub fn read_ghost_mesh(install: &Path, value: Option<&str>) -> Result<Mesh, GhostModelError> {
    let relative = normalize_value(value)?;
    let stem = &relative[..relative.len() - 4];
    let Some(mdl) = read_game_file(install, &relative)? else {
        return fail("ASSET_NOT_FOUND", "Model not found");
    };
    let vvd = read_game_file(install, &format!("{stem}.vvd"))?;
    let mut vtx = None;
    for suffix in [".dx90.vtx", ".dx80.vtx", ".sw.vtx"] {
        vtx = read_game_file(install, &format!("{stem}{suffix}"))?;
        if vtx.is_some() {
            break;
        }
    }
    let (Some(vvd), Some(vtx)) = (vvd, vtx) else {
        return fail("ASSET_NOT_FOUND", "Model files incomplete");
    };
    let mut mesh = parse_mesh(&mdl, &vvd, &vtx)?;
    for v in &mut mesh.positions {
        *v = js::math_round(*v * 100.0) / 100.0;
    }
    Ok(mesh)
}

/// `valueOfPicked(installPath, file)`: выбранный файл → значение `gh_model`. Только внутри мода
/// (`ghostingmod/models/…` или `ghostingmod/custom/<пак>/models/…`), иначе игра его не увидит.
/// Ссылки раскрываются: файл, который лишь лежит ссылкой в моде, игра читает снаружи.
pub fn value_of_picked(install: &Path, file: &Path) -> Result<String, GhostModelError> {
    let path = NodePath::native();
    let mod_dir = install.join("ghostingmod");
    let r#mod = real(&mod_dir).unwrap_or_else(|| path.resolve(&[&mod_dir.to_string_lossy()]));
    let full = real(file).unwrap_or_else(|| path.resolve(&[&file.to_string_lossy()]));
    let rest = full.get(r#mod.len() + 1..);
    let (true, Some(rest)) = (inside(Some(&full), Some(&r#mod)), rest) else {
        return fail("ASSET_OUTSIDE_MOD", "Model is outside the mod");
    };
    let relative = rest.split(path.sep()).collect::<Vec<_>>().join("/");
    // `/^custom\/[^/]+\/(.+)$/i`
    let in_pack = relative
        .get(..7)
        .filter(|head| head.eq_ignore_ascii_case("custom/"))
        .and_then(|_| relative[7..].split_once('/'))
        .filter(|(pack, inner)| !pack.is_empty() && !inner.is_empty())
        .map(|(_, inner)| inner);
    normalize_value(Some(in_pack.unwrap_or(&relative)))
}
