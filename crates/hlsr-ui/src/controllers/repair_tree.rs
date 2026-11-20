//! Окно ремонта по файлам (`RepairModal.vue` 0.4.9) без вида: файлы проверки с галочками,
//! разделы «сборка» и «cfg», больше 30 файлов в разделе — папки (галочка папки с промежуточным
//! состоянием, раскрытие шевроном), «Выбрать всё/Снять», размер выбранного, предупреждение, если
//! выбран изменённый игроком cfg. Изменённые cfg по умолчанию не отмечены. Ответ окна — пути
//! отмеченных файлов.

use hlsr_core::js;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Файл проверки: `{path, size, kind}`; `kind` — `missing | damaged | cfg-missing | cfg-changed`.
#[derive(Debug, Clone, PartialEq)]
pub struct RepairFile {
    pub path: String,
    pub size: f64,
    pub kind: String,
}

impl RepairFile {
    fn is_cfg(&self) -> bool {
        self.kind == "cfg-missing" || self.kind == "cfg-changed"
    }

    /// `tag(file)`: ключ подписи вида.
    pub fn tag_key(&self) -> &'static str {
        match self.kind.as_str() {
            "damaged" => "#L_GM_REPAIR_DAMAGED",
            "cfg-changed" => "#L_GM_REPAIR_CHANGED",
            _ => "#L_GM_REPAIR_MISSING",
        }
    }
}

/// Группа раздела: папка (`folder`) или весь раздел списком; `files` — номера в [`RepairTree::files`].
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub key: String,
    pub folder: bool,
    pub files: Vec<usize>,
}

/// Раздел окна: сборка (без заголовка) или cfg (`#L_GM_REPAIR_CFG`).
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub cfg: bool,
    pub groups: Vec<Group>,
}

/// `groupState(group)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupState {
    All,
    Some,
    None,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RepairTree {
    pub files: Vec<RepairFile>,
    chosen: BTreeMap<String, bool>,
    open: BTreeSet<String>,
}

impl RepairTree {
    /// `data()`: файлы ответа `game-menu-repair` (`check.files`).
    pub fn new(files: &[Value]) -> RepairTree {
        let files: Vec<RepairFile> = files
            .iter()
            .map(|file| RepairFile {
                path: js::to_template_string(file.get("path")),
                size: js::value::to_number(file.get("size")),
                kind: js::to_template_string(file.get("kind")),
            })
            .collect();
        let chosen = files
            .iter()
            .map(|file| (file.path.clone(), file.kind != "cfg-changed"))
            .collect();
        RepairTree {
            files,
            chosen,
            open: BTreeSet::new(),
        }
    }

    /// `sections`: пустые разделы не показываются.
    pub fn sections(&self) -> Vec<Section> {
        [false, true]
            .into_iter()
            .map(|cfg| Section {
                cfg,
                groups: self.groups_of(
                    (0..self.files.len())
                        .filter(|&i| self.files[i].is_cfg() == cfg)
                        .collect(),
                ),
            })
            .filter(|section| !section.groups.is_empty())
            .collect()
    }

    /// `groupsOf(files)`: больше 30 файлов — папки по `/`, по порядку `sort()`.
    fn groups_of(&self, files: Vec<usize>) -> Vec<Group> {
        if files.len() <= 30 {
            return if files.is_empty() {
                Vec::new()
            } else {
                vec![Group {
                    key: String::new(),
                    folder: false,
                    files,
                }]
            };
        }
        let mut by_dir: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for i in files {
            let path = &self.files[i].path;
            by_dir
                .entry(path.rfind('/').map_or("", |at| &path[..at]))
                .or_default()
                .push(i);
        }
        let mut groups: Vec<Group> = by_dir
            .into_iter()
            .map(|(key, files)| Group {
                key: key.to_owned(),
                folder: true,
                files,
            })
            .collect();
        groups.sort_by(|a, b| js::cmp_utf16(&a.key, &b.key));
        groups
    }

    /// Подпись файла в группе: в папке — без пути папки.
    pub fn name_in(&self, group: &Group, file: usize) -> String {
        let path = &self.files[file].path;
        if !group.folder {
            return path.clone();
        }
        let rest = path.get(group.key.len()..).unwrap_or_default();
        rest.strip_prefix('/').unwrap_or(rest).to_owned()
    }

    pub fn is_chosen(&self, file: usize) -> bool {
        self.chosen.get(&self.files[file].path) == Some(&true)
    }

    /// `toggle(path)`.
    pub fn toggle(&mut self, file: usize) {
        let path = self.files[file].path.clone();
        let next = !self.is_chosen(file);
        self.chosen.insert(path, next);
    }

    /// `setAll(value)`.
    pub fn set_all(&mut self, value: bool) {
        for file in &self.files {
            self.chosen.insert(file.path.clone(), value);
        }
    }

    pub fn group_state(&self, group: &Group) -> GroupState {
        let count = group.files.iter().filter(|&&i| self.is_chosen(i)).count();
        if count == group.files.len() {
            GroupState::All
        } else if count > 0 {
            GroupState::Some
        } else {
            GroupState::None
        }
    }

    /// `setGroup(group, value)`.
    pub fn set_group(&mut self, group: &Group, value: bool) {
        for &i in &group.files {
            self.chosen.insert(self.files[i].path.clone(), value);
        }
    }

    pub fn is_open(&self, key: &str) -> bool {
        self.open.contains(key)
    }

    /// `toggleOpen(key)`.
    pub fn toggle_open(&mut self, key: &str) {
        if !self.open.remove(key) {
            self.open.insert(key.to_owned());
        }
    }

    /// Отмеченные файлы (`chosenFiles`).
    fn chosen_files(&self) -> impl Iterator<Item = &RepairFile> {
        self.files
            .iter()
            .filter(|file| self.chosen.get(&file.path) == Some(&true))
    }

    /// Ответ окна: пути отмеченных файлов.
    pub fn chosen_paths(&self) -> Vec<String> {
        self.chosen_files().map(|file| file.path.clone()).collect()
    }

    pub fn chosen_count(&self) -> usize {
        self.chosen_files().count()
    }

    /// `sizeOf(files)`: `size || 0`.
    pub fn size_of<'a>(files: impl Iterator<Item = &'a RepairFile>) -> f64 {
        files
            .map(|file| if file.size.is_nan() { 0. } else { file.size })
            .sum()
    }

    pub fn group_size(&self, group: &Group) -> f64 {
        Self::size_of(group.files.iter().map(|&i| &self.files[i]))
    }

    pub fn chosen_bytes(&self) -> f64 {
        Self::size_of(self.chosen_files())
    }

    /// `warnConfigs`: среди отмеченных — изменённый игроком cfg.
    pub fn warn_configs(&self) -> bool {
        self.chosen_files().any(|file| file.kind == "cfg-changed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn folders_after_30_files_with_mixed_state() {
        let mut files: Vec<Value> = (0..31)
            .map(|i| json!({"path": format!("hl2/maps/m{i}.bsp"), "size": 10, "kind": "damaged"}))
            .collect();
        files[30] = json!({"path": "bin/client.dll", "size": 5, "kind": "missing"});
        files.push(json!({"path": "cfg/autoexec.cfg", "size": 1, "kind": "cfg-changed"}));
        let mut tree = RepairTree::new(&files);

        let sections = tree.sections();
        assert_eq!(sections.len(), 2);
        let keys: Vec<&str> = sections[0].groups.iter().map(|g| g.key.as_str()).collect();
        assert_eq!(keys, ["bin", "hl2/maps"]);
        // cfg — один файл, без папок; изменённый cfg не отмечен.
        assert!(!sections[1].groups[0].folder);
        assert!(!tree.warn_configs());
        assert_eq!(tree.chosen_count(), 31);
        assert_eq!(tree.chosen_bytes(), 305.);

        let maps = sections[0].groups[1].clone();
        assert_eq!(tree.name_in(&maps, maps.files[0]), "m0.bsp");
        assert_eq!(tree.group_state(&maps), GroupState::All);
        tree.toggle(maps.files[0]);
        assert_eq!(tree.group_state(&maps), GroupState::Some);
        tree.set_group(&maps, false);
        assert_eq!(tree.group_state(&maps), GroupState::None);
        tree.set_group(&maps, true);
        assert_eq!(tree.group_state(&maps), GroupState::All);

        let cfg = sections[1].groups[0].files[0];
        tree.toggle(cfg);
        assert!(tree.warn_configs());
        tree.set_all(false);
        assert_eq!(tree.chosen_count(), 0);
        assert!(tree.chosen_paths().is_empty());

        assert!(!tree.is_open("bin"));
        tree.toggle_open("bin");
        assert!(tree.is_open("bin"));
        tree.toggle_open("bin");
        assert!(!tree.is_open("bin"));
    }
}
