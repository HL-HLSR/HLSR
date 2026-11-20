//! Цель обновления (`update-target.js`): одинаковое описание версии из вшитого манифеста
//! игры или из подписанного манифеста канала.

use crate::js_value::{prop, truthy};
use serde_json::{Map, Value, json};

/// Цель обновления. Поля — значения манифеста как есть (`x || null` уже применено).
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateTarget {
    /// `local` или `channel`.
    pub source: &'static str,
    pub version: Value,
    pub update_required: bool,
    pub minimum_launcher_version: Value,
    pub published_at: Value,
    pub expected_installed_size: Value,
    pub patches: Value,
    pub full_archive: Value,
    pub repair_mirror: Value,
    pub files_manifest: Value,
    pub build_metadata: Value,
    pub notes: Value,
}

impl UpdateTarget {
    /// Объект цели как у 0.4.9 (те же ключи и порядок).
    pub fn to_json(&self) -> Value {
        let mut object = Map::new();
        object.insert("source".to_owned(), json!(self.source));
        object.insert("version".to_owned(), self.version.clone());
        object.insert("updateRequired".to_owned(), json!(self.update_required));
        object.insert(
            "minimumLauncherVersion".to_owned(),
            self.minimum_launcher_version.clone(),
        );
        object.insert("publishedAt".to_owned(), self.published_at.clone());
        object.insert(
            "expectedInstalledSize".to_owned(),
            self.expected_installed_size.clone(),
        );
        object.insert("patches".to_owned(), self.patches.clone());
        object.insert("fullArchive".to_owned(), self.full_archive.clone());
        object.insert("repairMirror".to_owned(), self.repair_mirror.clone());
        object.insert("filesManifest".to_owned(), self.files_manifest.clone());
        object.insert("buildMetadata".to_owned(), self.build_metadata.clone());
        object.insert("notes".to_owned(), self.notes.clone());
        Value::Object(object)
    }
}

/// `manifest[key] || fallback`.
fn or(manifest: &Value, key: &str, fallback: Value) -> Value {
    prop(manifest, key)
        .filter(|value| truthy(Some(value)))
        .cloned()
        .unwrap_or(fallback)
}

/// `typeof manifest.notes === "string" ? manifest.notes : null`.
fn notes_of(manifest: &Value) -> Value {
    match prop(manifest, "notes") {
        Some(notes @ Value::String(_)) => notes.clone(),
        _ => Value::Null,
    }
}

fn has_version(manifest: &Value) -> bool {
    truthy(prop(manifest, "version"))
}

/// `fromLocal(manifest)`: цель из вшитого манифеста игры; нет версии — `None`.
pub fn from_local(manifest: &Value) -> Option<UpdateTarget> {
    if !has_version(manifest) {
        return None;
    }
    Some(UpdateTarget {
        source: "local",
        version: or(manifest, "version", Value::Null),
        update_required: false,
        minimum_launcher_version: or(manifest, "minimumLauncherVersion", Value::Null),
        published_at: or(manifest, "publishedAt", Value::Null),
        expected_installed_size: or(manifest, "expectedInstalledSize", Value::Null),
        patches: or(manifest, "patches", json!([])),
        full_archive: or(manifest, "baseArchive", Value::Null),
        repair_mirror: Value::Null,
        files_manifest: or(manifest, "filesManifest", Value::Null),
        build_metadata: or(manifest, "buildMetadata", Value::Null),
        notes: notes_of(manifest),
    })
}

/// `fromChannel(manifest)`: цель из манифеста канала; нет версии — `None`.
pub fn from_channel(manifest: &Value) -> Option<UpdateTarget> {
    if !has_version(manifest) {
        return None;
    }
    Some(UpdateTarget {
        source: "channel",
        version: or(manifest, "version", Value::Null),
        update_required: truthy(prop(manifest, "updateRequired")),
        minimum_launcher_version: or(manifest, "minimumLauncherVersion", Value::Null),
        published_at: or(manifest, "publishedAt", Value::Null),
        expected_installed_size: or(manifest, "expectedInstalledSize", Value::Null),
        patches: or(manifest, "patches", json!([])),
        full_archive: or(manifest, "fullArchive", Value::Null),
        repair_mirror: or(manifest, "repairMirror", Value::Null),
        files_manifest: or(manifest, "filesManifest", Value::Null),
        build_metadata: or(manifest, "buildMetadata", Value::Null),
        notes: notes_of(manifest),
    })
}
