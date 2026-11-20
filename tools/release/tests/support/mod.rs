//! Общее для тестов упаковщика: деревья сборок во временных каталогах.

#![allow(dead_code)]

use hlsr_release::testing::{elf_stub, pe_with_version, png_stub, version_blob};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

pub fn write(root: &Path, relative: &str, bytes: &[u8]) -> PathBuf {
    let path = relative
        .split('/')
        .fold(root.to_owned(), |path, part| path.join(part));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, bytes).unwrap();
    path
}

pub fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `HLSR.exe` с ресурсом версии `product`.
pub fn exe_with_version(product: &str) -> Vec<u8> {
    let mut bytes = pe_with_version(Some(&version_blob(product)));
    bytes.extend_from_slice(b"\0code of the launcher");
    bytes
}

/// Ресурсы Windows-выпуска: 7za, оверлей, лицензии.
pub fn windows_resources(root: &Path) {
    write(root, "7zip/win/x64/7za.exe", b"7za");
    write(root, "7zip/LICENSE.txt", b"7zip license");
    write(root, "livesplit-overlay/hlsr-overlay.dll", b"overlay dll");
    write(
        root,
        "livesplit-components/LiveSplit.BunnySplit.dll",
        &[7u8; 70_000],
    );
    write(root, "licenses/HLSR-LICENSE.txt", "лицензия\n".as_bytes());
}

/// Сборка Linux: `hlsr`, значок, ресурсы, длинный путь, файл с кириллицей.
pub fn linux_tree(root: &Path) {
    write(root, "hlsr", &elf_stub("launcher"));
    write(root, "resources/icons/hlsr.png", &png_stub());
    write(root, "resources/7zip/linux/x64/7za", b"7za linux");
    write(
        root,
        "resources/linux/install-userns-profile.sh",
        b"#!/bin/sh\n",
    );
    write(root, "resources/Лицензия.txt", "текст".as_bytes());
    let deep = format!("resources/{}/{}", "d".repeat(60), "e".repeat(60));
    write(
        root,
        &format!("{deep}/file.bin"),
        b"deep file in a long path",
    );
}
