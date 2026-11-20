//! Файловая система Steam-модулей: настоящая (`RealFs`) и в памяти (`MemoryFs`), на которой
//! держатся сценарии `steam.rs`, — должны отвечать одинаково там, где это важно лаунчеру.

use hlsr_core::js::Platform;
use hlsr_platform::steam_fs::{MemoryFs, RealFs, SteamFs, read_to_string};

#[test]
fn real_fs_reads_like_node() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let root = root
        .to_string_lossy()
        .trim_start_matches("\\\\?\\")
        .to_owned();
    let file = format!("{root}{}config.txt", std::path::MAIN_SEPARATOR);
    std::fs::write(&file, "\u{feff}привет").unwrap();
    std::fs::create_dir(format!("{root}{}sub", std::path::MAIN_SEPARATOR)).unwrap();

    let fs = RealFs;
    assert!(fs.exists(&file));
    assert!(!fs.exists(&format!("{file}.none")));
    // BOM остаётся символом, как у `readFileSync(…, "utf8")`.
    assert_eq!(read_to_string(&fs, &file).unwrap(), "\u{feff}привет");

    let stat = fs.stat(&file).unwrap();
    assert!(stat.is_file && !stat.is_dir);
    assert_eq!(stat.size, "\u{feff}привет".len() as u64);
    let stat = fs.stat(&root).unwrap();
    assert!(stat.is_dir && !stat.is_file);
    assert!(fs.stat(&format!("{file}.none")).is_err());

    let mut names = fs.read_dir(&root).unwrap();
    names.sort();
    assert_eq!(names, ["config.txt", "sub"]);
    assert!(fs.read_dir(&file).is_err());
    assert!(
        fs.read_dir(&format!("{root}{}missing", std::path::MAIN_SEPARATOR))
            .is_err()
    );

    let real = fs.real_path(&file).unwrap();
    assert!(
        real.eq_ignore_ascii_case(&file),
        "{real} против {file}: без префикса \\\\?\\"
    );
    assert!(fs.real_path(&format!("{file}.none")).is_err());
}

#[test]
fn windows_memory_fs_ignores_case_and_creates_parents() {
    let fs = MemoryFs::new(Platform::Win32);
    fs.add_file("C:\\Program Files (x86)\\Steam\\steam.exe", "x");
    fs.add_dir("D:\\Empty");

    assert!(fs.exists("c:\\PROGRAM FILES (X86)\\steam\\STEAM.EXE"));
    assert!(fs.exists("C:/Program Files (x86)/Steam"));
    assert!(fs.exists("C:\\"));
    assert!(fs.exists("D:\\"), "корень диска есть всегда");
    assert!(!fs.exists("C:\\Program Files (x86)\\Steam\\other.exe"));
    assert!(fs.stat("C:\\Program Files (x86)").unwrap().is_dir);
    assert_eq!(
        fs.read_dir("c:\\program files (x86)\\steam").unwrap(),
        ["steam.exe"],
        "имена — как заданы, а не как запрошены"
    );
    assert!(fs.read_dir("D:\\Empty").unwrap().is_empty());
    assert!(
        fs.read("C:\\Program Files (x86)\\Steam").is_err(),
        "читать каталог нельзя"
    );
    assert!(
        fs.read_dir("C:\\Program Files (x86)\\Steam\\steam.exe")
            .is_err()
    );

    fs.remove("C:\\Program Files (x86)\\Steam\\steam.exe");
    assert!(!fs.exists("C:\\Program Files (x86)\\Steam\\steam.exe"));
    assert!(fs.exists("C:\\Program Files (x86)\\Steam"));
}

#[test]
fn linux_memory_fs_has_links_owners_and_case_sensitivity() {
    let fs = MemoryFs::new(Platform::Linux);
    fs.add_file("/data/Steam/steamapps/appmanifest_1.acf", "m");
    fs.add_link("/home/u/.steam/root", "/data/Steam");
    fs.add_link("/home/u/loop-a", "/home/u/loop-b");
    fs.add_link("/home/u/loop-b", "/home/u/loop-a");
    fs.add_dir("/proc/100");
    fs.set_uid("/proc/100", 1000);

    assert!(fs.exists("/home/u/.steam/root/steamapps"));
    assert!(!fs.exists("/data/steam"), "регистр важен");
    assert_eq!(fs.real_path("/home/u/.steam/root").unwrap(), "/data/Steam");
    assert_eq!(
        fs.real_path("/home/u/.steam/root/steamapps/../steamapps")
            .unwrap(),
        "/data/Steam/steamapps"
    );
    assert!(fs.real_path("/home/u/loop-a").is_err(), "петля ссылок");
    assert!(fs.real_path("/nowhere").is_err());
    assert_eq!(fs.stat("/proc/100").unwrap().uid, Some(1000));
    assert_eq!(fs.stat("/data").unwrap().uid, None);
    assert_eq!(
        fs.read_dir("/data/Steam").unwrap(),
        ["steamapps"],
        "по ссылке и по прямому пути список один"
    );
    assert_eq!(fs.read_dir("/home/u/.steam/root").unwrap(), ["steamapps"]);
    assert_eq!(
        fs.read("/home/u/.steam/root/steamapps/appmanifest_1.acf")
            .unwrap(),
        b"m"
    );
}
