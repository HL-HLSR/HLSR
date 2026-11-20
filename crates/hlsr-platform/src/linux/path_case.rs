//! Двойник папки по регистру (`linux/path-case.js`). Wine ищет файлы без учёта регистра, а Source
//! и GoldSrc обращаются к своим путям в нижнем регистре. Если рядом с папкой на пути к игре лежит
//! другая, отличающаяся только регистром (`~/Games/HLSR` и `~/Games/hlsr`), Wine уходит в неё
//! и игра падает на старте. Такая пара ищется до запуска.

use crate::steam_fs::SteamFs;
use hlsr_core::js::Platform;
use hlsr_core::js::path::NodePath;

/// Папка на пути к игре и её двойник.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseConflict {
    pub folder: String,
    pub twin: String,
}

/// `findCaseConflict(target, fs)`: первая (от корня) папка пути, у которой в родителе есть двойник.
pub fn find_case_conflict(target: &str, fs: &dyn SteamFs) -> Option<CaseConflict> {
    let path = NodePath::new(Platform::Linux, "/");
    let mut folders = Vec::new();
    let mut dir = path.resolve(&[target]);
    while path.dirname(&dir) != dir {
        folders.push(dir.clone());
        dir = path.dirname(&dir);
    }
    folders.reverse();

    for folder in folders {
        let parent = path.dirname(&folder);
        let name = path.basename(&folder);
        let Ok(entries) = fs.read_dir(&parent) else {
            continue;
        };
        let lower = name.to_lowercase();
        if let Some(twin) = entries
            .iter()
            .find(|entry| **entry != name && entry.to_lowercase() == lower)
        {
            return Some(CaseConflict {
                folder,
                twin: path.join(&[&parent, twin]),
            });
        }
    }
    None
}
