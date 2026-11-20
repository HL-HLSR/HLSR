//! Панель инструмента (`ToolPanel.vue` 0.4.9): герой (название, описание, размер загрузки) и
//! нижняя панель (отмена установки, справка, папка, удаление, главное действие, полоса
//! установки) по карточке `ToolCard` каналов `tool-list` и `tool-status`.

use super::boot::format_size;
use super::jsv;
use crate::tr::tr;
use hlsr_core::js;
use hlsr_core::language::Language;
use serde_json::Value;

/// Что нажатие главной кнопки передаёт оболочке (`onAction`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolAction {
    Stop,
    Launch,
    Install,
}

impl ToolAction {
    /// Событие компонента (`stop`, `launch`, `install`).
    pub fn name(self) -> &'static str {
        match self {
            ToolAction::Stop => "stop",
            ToolAction::Launch => "launch",
            ToolAction::Install => "install",
        }
    }
}

/// Что показывает панель инструмента.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolView {
    pub tool_id: String,
    pub title: String,
    /// `$t(tool.subtitle)` или пусто (подзаголовка нет).
    pub subtitle: String,
    pub description: String,
    /// «Скачать: 8,0 МБ» — только пока инструмент не установлен.
    pub size_text: String,
    pub installed: bool,
    pub running: bool,
    pub busy: bool,
    /// Полоса установки, 0…100.
    pub percent: f64,
    /// Крестик «Отмена» — идёт установка.
    pub show_cancel: bool,
    pub has_help: bool,
    pub action_label: String,
    pub action_icon: &'static str,
    /// Тон «прогресс» (синий) вместо «готово»: установка и обновление.
    pub action_progress: bool,
    /// Что даст нажатие; `None` — кнопка занята.
    pub action: Option<ToolAction>,
}

/// Карточка по умолчанию, когда инструментов нет (`currentTool`).
pub fn empty_tool() -> Value {
    serde_json::json!({"toolId": "", "title": "", "state": "unknown", "icon": "fas fa-toolbox"})
}

/// `currentTool` оболочки: выбранный инструмент, иначе первый, иначе пустая карточка.
pub fn current_tool(tools: &[Value], selected: &str) -> Value {
    tools
        .iter()
        .find(|tool| jsv::is_str(jsv::prop(tool, "toolId"), selected))
        .or(tools.first())
        .cloned()
        .unwrap_or_else(empty_tool)
}

fn flag(tool: &Value, name: &str) -> bool {
    jsv::prop(tool, name) == Some(&Value::Bool(true))
}

fn text(tool: &Value, name: &str) -> String {
    jsv::str_of(jsv::prop(tool, name))
        .unwrap_or_default()
        .to_owned()
}

/// Ключ локализации из карточки → текст (`$t(tool.subtitle)`); пусто, если поля нет.
fn translated(lang: Language, tool: &Value, name: &str) -> String {
    match jsv::prop(tool, name).filter(|value| js::truthy(value)) {
        Some(value) => tr(lang, &js::to_string(value), &[]),
        None => String::new(),
    }
}

/// `sizeText` 0.4.9: `formatSize` — единица по размеру, знак дроби языка («8,0 МБ»).
fn size_text(lang: Language, tool: &Value) -> String {
    let bytes = jsv::or_zero(jsv::prop(tool, "sizeBytes"));
    format_size(bytes, &|key, args| tr(lang, key, args))
}

/// Панель по карточке инструмента.
pub fn view(lang: Language, tool: &Value) -> ToolView {
    let installed = jsv::is_str(jsv::prop(tool, "state"), "ready");
    let running = flag(tool, "running");
    let stopping = flag(tool, "stopping");
    let busy = jsv::is_str(jsv::prop(tool, "state"), "installing") || stopping;
    let update_available = jsv::truthy(jsv::prop(tool, "updateAvailable"));
    let percent = jsv::or_zero(jsv::prop(tool, "percent")).clamp(0.0, 100.0);

    let action_label = if stopping {
        tr(lang, "#L_TOOL_STOPPING", &[])
    } else if running {
        tr(lang, "#L_CLOSE", &[])
    } else if busy {
        tr(lang, "#L_TOOL_INSTALLING", &[])
    } else if update_available {
        tr(lang, "#L_TOOL_UPDATE", &[])
    } else {
        tr(
            lang,
            if installed {
                "#L_TOOL_RUN"
            } else {
                "#L_TOOL_INSTALL"
            },
            &[],
        )
    };
    let action_icon = if busy {
        "fas fa-spinner fa-spin"
    } else if running {
        "fas fa-stop"
    } else if update_available {
        "fas fa-arrow-up"
    } else if installed {
        "fas fa-play"
    } else {
        "fas fa-download"
    };
    let action = if busy {
        None
    } else if running {
        Some(ToolAction::Stop)
    } else if installed && !update_available {
        Some(ToolAction::Launch)
    } else {
        Some(ToolAction::Install)
    };
    let sized = jsv::truthy(jsv::prop(tool, "sizeBytes")) && !installed;

    ToolView {
        tool_id: text(tool, "toolId"),
        title: text(tool, "title"),
        subtitle: translated(lang, tool, "subtitle"),
        description: translated(lang, tool, "description"),
        size_text: if sized {
            tr(lang, "#L_TOOL_SIZE", &[&size_text(lang, tool)])
        } else {
            String::new()
        },
        installed,
        running,
        busy,
        percent,
        show_cancel: jsv::is_str(jsv::prop(tool, "state"), "installing"),
        has_help: jsv::truthy(jsv::prop(tool, "hasHelp")),
        action_label,
        action_icon,
        action_progress: !running && (!installed || update_available),
        action,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// LiveSplit из ответа `tool-list` (фикстура сцен).
    fn livesplit(extra: Value) -> Value {
        let mut tool = json!({
            "toolId": "livesplit",
            "state": "not-installed",
            "version": null,
            "availableVersion": "1.0.0",
            "updateAvailable": false,
            "installPath": null,
            "percent": 0,
            "error": null,
            "running": false,
            "stopping": false,
            "shortName": "LiveSplit",
            "title": "LiveSplit",
            "subtitle": "#L_TOOL_LIVESPLIT_SUB",
            "description": "#L_TOOL_LIVESPLIT_DESC",
            "icon": "fas fa-stopwatch",
            "accent": "#3fa7ff",
            "hasHelp": true,
            "sizeBytes": 8_363_929,
        });
        for (key, value) in extra.as_object().unwrap() {
            tool[key] = value.clone();
        }
        tool
    }

    fn en(extra: Value) -> ToolView {
        view(Language::En, &livesplit(extra))
    }

    #[test]
    fn the_hero_shows_title_description_and_download_size_until_installed() {
        let view = en(json!({}));
        assert_eq!(view.title, "LiveSplit");
        assert_eq!(view.subtitle, "Speedrun timer");
        assert!(
            view.description
                .starts_with("Times your run and marks each segment")
        );
        // 8 363 929 байт — 8.0 MB; между числом и единицей неразрывный пробел (formatSize).
        assert_eq!(view.size_text, "Download: 8.0\u{a0}MB");
        assert_eq!(
            en(json!({"state": "installing"})).size_text,
            "Download: 8.0\u{a0}MB"
        );
        assert_eq!(
            en(json!({"sizeBytes": 900})).size_text,
            "Download: 900\u{a0}B"
        );
        assert_eq!(
            en(json!({"sizeBytes": 1_572_864})).size_text,
            "Download: 1.5\u{a0}MB"
        );
        assert_eq!(en(json!({"state": "ready"})).size_text, "");
        assert_eq!(en(json!({"sizeBytes": 0})).size_text, "");
        assert_eq!(en(json!({"sizeBytes": null})).size_text, "");
        let russian = view_ru(json!({}));
        assert_eq!(russian.subtitle, "Таймер для спидрана");
        // Знак дроби — запятая языка.
        assert_eq!(russian.size_text, "Скачать: 8,0\u{a0}МБ");
        assert_eq!(
            view_ru(json!({"sizeBytes": 2_684_354_560u64})).size_text,
            "Скачать: 2,5\u{a0}ГБ"
        );
        // Нет подзаголовка и описания — пусто.
        let bare = en(json!({"subtitle": null, "description": ""}));
        assert_eq!(
            (bare.subtitle.as_str(), bare.description.as_str()),
            ("", "")
        );
    }

    fn view_ru(extra: Value) -> ToolView {
        view(Language::Ru, &livesplit(extra))
    }

    #[test]
    fn the_main_action_follows_the_tool_state() {
        // (карточка, подпись, значок, тон «прогресс», действие)
        let cases = [
            (
                json!({}),
                "Install",
                "fas fa-download",
                true,
                Some(ToolAction::Install),
            ),
            (
                json!({"state": "unknown"}),
                "Install",
                "fas fa-download",
                true,
                Some(ToolAction::Install),
            ),
            (
                json!({"state": "installing", "percent": 40}),
                "Installing…",
                "fas fa-spinner fa-spin",
                true,
                None,
            ),
            (
                json!({"state": "ready"}),
                "Run",
                "fas fa-play",
                false,
                Some(ToolAction::Launch),
            ),
            (
                json!({"state": "ready", "updateAvailable": true}),
                "Update",
                "fas fa-arrow-up",
                true,
                Some(ToolAction::Install),
            ),
            (
                json!({"state": "ready", "running": true}),
                "Close",
                "fas fa-stop",
                false,
                Some(ToolAction::Stop),
            ),
            (
                json!({"state": "ready", "running": true, "updateAvailable": true}),
                "Close",
                "fas fa-stop",
                false,
                Some(ToolAction::Stop),
            ),
            (
                json!({"state": "ready", "running": true, "stopping": true}),
                "Closing…",
                "fas fa-spinner fa-spin",
                false,
                None,
            ),
        ];
        for (card, label, icon, progress, action) in cases {
            let view = en(card.clone());
            assert_eq!(view.action_label, label, "{card}");
            assert_eq!(view.action_icon, icon, "{card}");
            assert_eq!(view.action_progress, progress, "{card}");
            assert_eq!(view.action, action, "{card}");
        }
        assert_eq!(ToolAction::Launch.name(), "launch");
    }

    #[test]
    fn the_progress_is_clamped_and_cancel_shows_only_while_installing() {
        assert_eq!(
            en(json!({"state": "installing", "percent": 40})).percent,
            40.0
        );
        assert_eq!(en(json!({"percent": 240})).percent, 100.0);
        assert_eq!(en(json!({"percent": -3})).percent, 0.0);
        assert_eq!(en(json!({"percent": null})).percent, 0.0);
        assert!(en(json!({"state": "installing"})).show_cancel);
        assert!(!en(json!({"state": "ready", "stopping": true})).show_cancel);
        assert!(en(json!({"state": "ready", "stopping": true})).busy);
        assert!(!en(json!({"state": "ready"})).busy);
    }

    #[test]
    fn help_folder_and_remove_flags() {
        assert!(en(json!({})).has_help);
        assert!(!en(json!({"hasHelp": false})).has_help);
        let ready = en(json!({"state": "ready", "running": true}));
        assert!(ready.installed && ready.running && !ready.busy);
        assert!(!en(json!({})).installed);
    }

    #[test]
    fn the_current_tool_is_the_selected_then_the_first() {
        let tools = [livesplit(json!({})), json!({"toolId": "other"})];
        assert_eq!(current_tool(&tools, "other")["toolId"], "other");
        assert_eq!(current_tool(&tools, "")["toolId"], "livesplit");
        assert_eq!(current_tool(&tools, "gone")["toolId"], "livesplit");
        assert_eq!(current_tool(&[], "livesplit"), empty_tool());
    }

    #[test]
    fn with_no_tools_the_card_is_empty() {
        let view = view(Language::En, &empty_tool());
        assert_eq!(view.tool_id, "");
        assert_eq!(view.title, "");
        assert_eq!(view.action_label, "Install");
        assert_eq!(view.size_text, "");
        assert!(!view.has_help);
    }
}
