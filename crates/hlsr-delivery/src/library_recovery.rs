//! Где лежат игры (`library-recovery.js`): если по сохранённому пути библиотеки нет ни одной
//! установленной игры (`installed.json` с `state: "ready"`), а в стандартной папке они есть,
//! лаунчер работает со стандартной — библиотеку перенесли или путь в настройках устарел.

use crate::paths::resolve_library;
use crate::ports::Logger;
use hlsr_core::js;
use hlsr_core::js::path::NodePath;
use hlsr_core::json_text::json_parse;
use std::path::Path;

/// `INSTALLED_FILE`.
pub const INSTALLED_FILE: &str = "installed.json";

/// `installedGamesAt(root, { gameIds })`: сколько игр из `game_ids` установлено в библиотеке
/// `root`. Путь, который не годится в библиотеку, — 0.
pub fn installed_games_at(root: &str, game_ids: &[&str]) -> usize {
    if root.is_empty() {
        return 0;
    }
    let Ok(layout) = resolve_library(root) else {
        return 0;
    };
    let path = NodePath::native();
    let games = layout.games.to_string_lossy();
    game_ids
        .iter()
        .filter(|game_id| {
            let file = path.join(&[&games, game_id, INSTALLED_FILE]);
            // JSON.parse(fs.readFileSync(file, "utf8")): BOM не срезается.
            js::fs::read_file(Path::new(&file))
                .ok()
                .and_then(|bytes| json_parse(&js::utf8_string(&bytes)).ok())
                .is_some_and(|data| {
                    js::truthy(&data) && data.get("state").and_then(|s| s.as_str()) == Some("ready")
                })
        })
        .count()
}

/// `samePath(a, b)`: пути совпадают после `path.resolve` без учёта регистра.
fn same_path(a: &str, b: &str) -> bool {
    let path = NodePath::native();
    path.resolve(&[a]).to_lowercase() == path.resolve(&[b]).to_lowercase()
}

/// `recoverLibraryPath(current, { fallback, gameIds, logger })`.
pub fn recover_library_path(
    current: Option<&str>,
    fallback: Option<&str>,
    game_ids: &[&str],
    logger: &dyn Logger,
) -> Option<String> {
    recover_library_path_with(current, fallback, logger, |root| {
        installed_games_at(root, game_ids)
    })
}

/// `recoverLibraryPath` с подменённым подсчётом игр (`countAt`).
pub fn recover_library_path_with(
    current: Option<&str>,
    fallback: Option<&str>,
    logger: &dyn Logger,
    count_at: impl Fn(&str) -> usize,
) -> Option<String> {
    let (Some(path), Some(fallback)) = (
        current.filter(|c| !c.is_empty()),
        fallback.filter(|f| !f.is_empty()),
    ) else {
        return current.map(str::to_owned);
    };
    if same_path(path, fallback) || count_at(path) > 0 {
        return Some(path.to_owned());
    }
    let found = count_at(fallback);
    if found == 0 {
        return Some(path.to_owned());
    }
    logger.warn(&format!(
        "[game-delivery] по сохранённому пути библиотеки игр нет ({path}), \
         а в стандартном найдено {found} — работаем с {fallback}"
    ));
    Some(fallback.to_owned())
}
