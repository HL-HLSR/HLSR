// Ресурсы Windows: значок (GPUI берёт его же для окна, ресурс 1) и манифест hlsr-app.
// Список картинок `crates/hlsr-ui/assets/**` для `assets.rs` (вшиваются в exe, как в Electron).

use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../hlsr-app/hlsr.manifest");
    println!("cargo:rerun-if-changed=../../build/icons/icon.ico");
    embed_assets();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../build/icons/icon.ico")
        .set_manifest_file("../hlsr-app/hlsr.manifest");
    res.compile().expect("ресурсы Windows не собрались");
}

/// `$OUT_DIR/assets.rs`: `&[("glyphs/folder.svg", include_bytes!(…)), …]` по имени.
fn embed_assets() {
    // Переменная при запуске, не env!: target/ бывает общим с worktree, и старый build.rs смотрел бы в чужое дерево.
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let root = Path::new(&manifest).join("../hlsr-ui/assets");
    // Каталог: cargo сам обходит его целиком и пересобирает при любой правке внутри.
    println!("cargo:rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    walk(&root, &mut files);
    files.sort();
    let mut out = String::from("&[\n");
    for file in &files {
        let name = file.strip_prefix(&root).expect("файл вне assets");
        let name = name.to_string_lossy().replace('\\', "/");
        let path = file.canonicalize().expect("путь картинки");
        out.push_str(&format!("    ({name:?}, include_bytes!({path:?})),\n"));
    }
    out.push_str("]\n");
    let dest = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("assets.rs");
    std::fs::write(dest, out).expect("assets.rs не записан");
}

fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("каталог assets") {
        let path = entry.expect("запись assets").path();
        if path.is_dir() {
            walk(&path, files);
        } else {
            files.push(path);
        }
    }
}
