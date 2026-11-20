// Ресурсы Windows: значок, VERSIONINFO и манифест. Только для цели Windows.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=hlsr.manifest");
    println!("cargo:rerun-if-changed=../../build/icons/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    // 0.5.0-dev → 0.5.0.0: в VERSIONINFO только числа.
    let numeric = numeric_version(env!("CARGO_PKG_VERSION"));
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../build/icons/icon.ico")
        .set_manifest_file("hlsr.manifest")
        .set("ProductName", "HLSR")
        .set("CompanyName", "HLSR Team")
        .set("FileDescription", "HLSR")
        .set("InternalName", "HLSR")
        .set("OriginalFilename", "HLSR.exe")
        .set("LegalCopyright", "Copyright © 2020-2026 HLSR Team")
        .set("FileVersion", &numeric.0)
        .set("ProductVersion", &numeric.0)
        .set_version_info(winresource::VersionInfo::FILEVERSION, numeric.1)
        .set_version_info(winresource::VersionInfo::PRODUCTVERSION, numeric.1);
    res.compile().expect("ресурсы Windows не собрались");
}

fn numeric_version(version: &str) -> (String, u64) {
    let core = version.split(['-', '+']).next().unwrap_or(version);
    let mut parts = [0u64; 4];
    for (slot, part) in parts.iter_mut().zip(core.split('.')) {
        *slot = part.parse().unwrap_or(0);
    }
    let text = parts.map(|p| p.to_string()).join(".");
    (
        text,
        parts[0] << 48 | parts[1] << 32 | parts[2] << 16 | parts[3],
    )
}
