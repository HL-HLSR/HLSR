//! Самотесты 0.4.7 о биндах и гайде HL1 (`scripts/hl1-selftest.js`) с теми же случаями.
//! Здесь только то, что опирается на `game-binds.js`, `bind-presets.js` и
//! `hl1-guide.js`; части тестов, которые пишут и читают файлы игры
//! (`game-binds-install`, `goldsrc-personalization`, `cfg-import`), — в задаче 13.

use hlsr_core::js::Platform;
use hlsr_core::js::object::JsObject;
use hlsr_delivery::bind_presets::{self, AliasInfo};
use hlsr_delivery::game_binds::{
    self, Bind, ImportOptions, Override, Overrides, command_error, normalize_key,
};
use hlsr_delivery::hl1_guide;
use serde_json::json;
use std::collections::HashMap;

fn files(list: &[(&str, &str)]) -> HashMap<String, String> {
    list.iter()
        .map(|(n, t)| ((*n).to_owned(), (*t).to_owned()))
        .collect()
}

fn bind(command: &str, source: &str) -> Bind {
    Bind {
        command: command.into(),
        source: source.into(),
    }
}

fn aliases(list: &[(&str, &str)]) -> JsObject<String> {
    list.iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn overrides(list: &[(&str, &str, &str)]) -> Overrides {
    list.iter()
        .map(|(k, c, b)| {
            (
                (*k).to_owned(),
                Override {
                    command: (*c).into(),
                    base: (*b).into(),
                },
            )
        })
        .collect()
}

fn keys<V>(object: &JsObject<V>) -> Vec<&str> {
    object.keys().collect()
}

#[test]
fn hl1_cfg_is_parsed_like_the_engine_exec_chain_and_unbindall() {
    let all = files(&[
        (
            "config.cfg",
            &[
                "unbindall",
                "bind \"w\" \"+forward\"",
                "bind \"SPACE\" \"+jump\" // комментарий",
                "bind \"q\" \"lastinv\"; bind \"E\" \"+use\"",
                "alias \"jb\" \"+jump;wait;-jump\"",
                "exec userconfig.cfg",
            ]
            .join("\r\n"),
        ),
        (
            "userconfig.cfg",
            "bind \"mwheeldown\" \"jb\"\nunbind \"q\"\nexec scripts.cfg\nexec hlsr-binds.cfg\nexec config.cfg",
        ),
        ("scripts.cfg", "bind \"f\" \"bxt_append; +duck\""),
        ("hlsr-binds.cfg", "bind \"w\" \"+back\""),
    ]);
    let result = game_binds::read_binds(|name| all.get(name).cloned(), game_binds::GOLDSRC.roots);
    assert_eq!(
        result.binds.get("w").unwrap().command,
        "+forward",
        "свой hlsr-binds.cfg лаунчер не читает"
    );
    assert_eq!(result.binds.get("SPACE").unwrap().command, "+jump");
    assert_eq!(
        result.binds.get("e").unwrap().command,
        "+use",
        "имя из одной буквы приводится к нижнему регистру"
    );
    assert!(result.binds.get("q").is_none(), "unbind снимает бинд");
    assert_eq!(
        result.binds.get("MWHEELDOWN").unwrap().source,
        "userconfig.cfg"
    );
    assert_eq!(result.binds.get("f").unwrap().command, "bxt_append; +duck");
    assert_eq!(result.aliases.get("jb").unwrap(), "+jump;wait;-jump");
    assert_eq!(
        result.files,
        ["config.cfg", "userconfig.cfg", "scripts.cfg"],
        "повторный exec не зацикливает"
    );
}

#[test]
fn hl1_import_moves_binds_unbindall_clears_the_rest_alias_from_file_is_skipped() {
    let file_binds: JsObject<Bind> = [
        ("w", bind("+forward", "config.cfg")),
        ("e", bind("+use", "config.cfg")),
        ("SPACE", bind("+jump", "config.cfg")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    let text = [
        "unbindall",
        "alias +jb \"+jump\"",
        "bind \"w\" \"+forward\"",
        "bind \"SPACE\" \"+jb\"",
        "bind \"f\" \"impulse 100\" // фонарик",
        "bind \"MWHEELDOWN\" \"+jump\"",
        "bind \"NOSUCHKEY\" \"x\"",
    ]
    .join("\n");
    let none = Overrides::new();
    let no_aliases = JsObject::new();
    let options = ImportOptions {
        file_binds: Some(&file_binds),
        overrides: Some(&none),
        aliases: Some(&no_aliases),
        ..ImportOptions::default()
    };
    let result = game_binds::import_overrides(&text, &options);
    let expected = overrides(&[
        ("f", "impulse 100", ""),
        ("MWHEELDOWN", "+jump", ""),
        ("e", "", "+use"),
    ]);
    assert_eq!(result.overrides, expected);
    assert_eq!(result.changed, 2);
    assert_eq!(result.unbound, 1);
    assert_eq!(result.unchanged, 1);
    assert_eq!(result.skipped_alias, 1);
    assert_eq!(result.skipped_key, 1);

    // alias, который уже есть в игре, или команда заготовки лаунчера — не пропуск.
    let game_aliases = aliases(&[("+JB", "+jump")]);
    let known = game_binds::import_overrides(
        &text,
        &ImportOptions {
            file_binds: Some(&file_binds),
            overrides: Some(&none),
            known_command: None,
            aliases: Some(&game_aliases),
        },
    );
    assert_eq!(
        known.overrides.get("SPACE"),
        Some(&Override {
            command: "+jb".into(),
            base: "+jump".into()
        })
    );

    // Повторный импорт того же файла ничего не меняет.
    let again = game_binds::import_overrides(
        &text,
        &ImportOptions {
            file_binds: Some(&file_binds),
            overrides: Some(&result.overrides),
            aliases: Some(&no_aliases),
            ..ImportOptions::default()
        },
    );
    assert_eq!(again.overrides, result.overrides);
    assert_eq!(again.changed + again.unbound, 0);
}

#[test]
fn hl1_overrides_are_written_to_binds_cfg_and_returns_are_pruned() {
    let input = json!({
        "W": {"command": "+back", "base": "+forward"},
        "q": {"command": "", "base": "lastinv"},
        "bad\"key": {"command": "+jump", "base": ""},
        "e": {"command": "say \"hi\"", "base": "+use"},
    });
    let clean = game_binds::sanitize_overrides(&input);
    let mut names = keys(&clean);
    names.sort_unstable();
    assert_eq!(
        names,
        ["q", "w"],
        "кавычки в клавише и команде отбрасываются"
    );
    let cfg = game_binds::to_cfg_lines(&clean, None, &[]);
    assert!(cfg.contains("bind \"w\" \"+back\""));
    assert!(cfg.contains("unbind \"q\""));

    let reset = game_binds::reset_overrides(&clean);
    assert_eq!(
        reset.get("w"),
        Some(&Override {
            command: "+forward".into(),
            base: "+forward".into()
        })
    );
    // Игра после выхода записала прежний бинд в config.cfg: возврат больше не нужен.
    let in_game: JsObject<Bind> = [
        ("w".to_owned(), bind("+forward", "")),
        ("q".to_owned(), bind("+use", "")),
    ]
    .into_iter()
    .collect();
    assert_eq!(keys(&game_binds::prune_overrides(&reset, &in_game)), ["q"]);
}

#[test]
fn script_presets_of_hl1_are_marked_for_scripted_other_commands_are_not() {
    for platform in [Platform::Linux, Platform::Win32] {
        let hl1 = bind_presets::catalog_for("hl1-won", platform);
        let none = AliasInfo::default();
        assert!(hl1.unwrap().run_modes);
        let scripted = |command: &str| bind_presets::is_scripted(hl1, command, &none);
        assert!(scripted("+fps_20_hold"));
        assert!(scripted("+bxt_tas_ducktap"));
        assert!(!scripted("fps_max 20"));
        assert!(!scripted("+jump"));
        assert!(
            scripted("+jump; wait; -jump"),
            "своя команда из нескольких — скрипт"
        );
        assert_eq!(
            hl1.unwrap().preset_of("SAVE X;load x").unwrap().label.en,
            "Save & load",
            "пробелы и регистр не важны"
        );
        for id in bind_presets::GAME_IDS {
            let catalog = bind_presets::catalog_for(id, platform).unwrap();
            for preset in &catalog.presets {
                assert!(
                    catalog.categories.iter().any(|c| c.id == preset.category),
                    "{id}: {}",
                    preset.command
                );
                assert_eq!(
                    command_error(&preset.command),
                    None,
                    "{id}: {}",
                    preset.command
                );
                for (_, value) in &preset.aliases {
                    assert!(!value.contains('"'), "{id}: {}", preset.command);
                }
            }
        }
        assert!(
            !bind_presets::catalog_for("ghosting-renewed", platform)
                .unwrap()
                .run_modes
        );
        let steampipe = if platform == Platform::Linux {
            "save /;reload"
        } else {
            "save :;reload"
        };
        assert_eq!(
            bind_presets::steampipe_save_deletion(platform),
            steampipe,
            "под Linux удаление сейва через «save /»"
        );
        assert!(
            bind_presets::catalog_for("hl2-5288", platform)
                .unwrap()
                .preset_of(steampipe)
                .is_some()
        );
        let pre = bind_presets::pre_steampipe_save_deletion();
        assert!(
            bind_presets::catalog_for("hl2-4260", platform)
                .unwrap()
                .preset_of(&pre)
                .is_some()
        );
    }
}

#[test]
fn guide_every_action_is_an_hl1_preset_and_script_aliases_do_not_conflict() {
    let hl1 = bind_presets::catalog_for("hl1-won", Platform::Linux).unwrap();
    assert!(
        hl1_guide::scripts().len() >= 70,
        "в гайде больше 70 скриптов"
    );
    let mut values: HashMap<&str, &str> = HashMap::new();
    for preset in &hl1.presets {
        for (name, value) in &preset.aliases {
            if let Some(known) = values.get(name.as_str()) {
                assert_eq!(
                    known, value,
                    "alias {name} в двух заготовках с разным кодом"
                );
            }
            values.insert(name, value);
        }
    }
    for script in hl1_guide::scripts() {
        assert!(!script.actions.is_empty(), "{}: нет действий", script.id);
        assert!(
            hl1_guide::chapters().iter().any(|c| c.id == script.chapter),
            "{}",
            script.id
        );
        for action in &script.actions {
            assert!(
                hl1.preset_of(&action.command).is_some(),
                "{}: {}",
                script.id,
                action.command
            );
            assert_eq!(
                command_error(&action.command),
                None,
                "{}: {}",
                script.id,
                action.command
            );
            if let Some(key) = &action.guide_key {
                assert!(normalize_key(key).is_some(), "{}: клавиша {key}", script.id);
            }
        }
        // Своё действие скрипта приносит все его alias. Общие команды вроде
        // +bxt_tas_jumpbug остаются прежними заготовками без alias.
        let own: Vec<_> = script
            .actions
            .iter()
            .filter(|action| {
                let preset = hl1.preset_of(&action.command).unwrap();
                preset.guide.as_deref() == Some(script.id.as_str()) || !preset.aliases.is_empty()
            })
            .collect();
        assert!(
            !own.is_empty(),
            "{}: ни одно действие не приносит alias",
            script.id
        );
        for action in own {
            let set = overrides(&[("k", &action.command, "")]);
            let written: HashMap<String, String> =
                bind_presets::aliases_for("hl1-won", Platform::Linux, &set)
                    .into_iter()
                    .collect();
            for (name, value) in &script.aliases {
                assert_eq!(
                    written.get(name),
                    Some(value),
                    "{}: alias {name}",
                    script.id
                );
            }
        }
        if let Some(video) = &script.video {
            assert!(
                video.id.len() == 11
                    && video
                        .id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
                "{}",
                script.id
            );
        }
    }
    let guide: Vec<_> = hl1
        .presets
        .iter()
        .filter(|p| p.category == "guide")
        .collect();
    assert!(
        !guide.is_empty() && guide.iter().all(|p| !p.listed && p.scripted),
        "в списке клавиатуры их нет, отмечены как скрипты"
    );
}

#[test]
fn guide_sensitivity_and_pitch_of_the_player_fall_back_to_defaults() {
    assert_eq!(hl1_guide::clean_sensitivity(Some("0")), "3");
    assert_eq!(hl1_guide::clean_sensitivity(Some("1.5")), "1.5");
    assert_eq!(hl1_guide::clean_sensitivity(Some("2;quit")), "3");
    assert_eq!(hl1_guide::clean_pitch(None), "0.022");
    assert_eq!(hl1_guide::clean_pitch(Some("0")), "0.022");
    assert_eq!(hl1_guide::clean_pitch(Some("0.022;quit")), "0.022");
    assert_eq!(hl1_guide::clean_pitch(Some("-0.03")), "-0.03");
    let cvars = aliases(&[("sensitivity", "2.25"), ("m_pitch", "-0.022")]);
    assert_eq!(
        hl1_guide::with_player_cvars("sensitivity {sensitivity};m_pitch {m_pitch}", &cvars),
        "sensitivity 2.25;m_pitch -0.022"
    );
}

#[test]
fn guide_only_links_from_the_guide_open_outside() {
    let video = hl1_guide::scripts()
        .iter()
        .find_map(|s| s.video.as_ref())
        .unwrap()
        .id
        .clone();
    let save = hl1_guide::scripts()
        .iter()
        .find_map(|s| s.save.clone())
        .unwrap();
    let repo = &hl1_guide::source().repo;
    assert!(hl1_guide::is_guide_link(&format!("{repo}#readme")));
    assert!(hl1_guide::is_guide_link(&save));
    assert!(hl1_guide::is_guide_link(&format!(
        "https://www.youtube.com/watch?v={video}"
    )));
    assert!(hl1_guide::is_guide_link(&format!(
        "https://www.youtube.com/watch?v={video}&t=42"
    )));
    assert!(
        !hl1_guide::is_guide_link("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
        "чужое видео"
    );
    assert!(!hl1_guide::is_guide_link(&format!(
        "https://www.youtube.com/watch?v={video}&list=x"
    )));
    assert!(!hl1_guide::is_guide_link("https://example.com/"));
    assert!(!hl1_guide::is_guide_link("file:///etc/passwd"));
}

#[test]
fn guide_russian_translation_exists_for_all_and_keeps_links_and_code() {
    // Те же регулярки, что в hl1-selftest.js: `/\]\(([^)\s]+)\)/g` и `/`([^`]+)`/g`.
    fn captures(pattern: &str, text: &str) -> Vec<String> {
        let regex = regress::Regex::new(pattern).unwrap();
        let mut found: Vec<String> = regex
            .find_iter(text)
            .map(|m| text[m.group(1).unwrap()].to_owned())
            .collect();
        found.sort_by(|a, b| hlsr_core::js::cmp_utf16(a, b));
        found
    }
    let links = |text: &str| captures(r"\]\(([^)\s]+)\)", text);
    let code = |text: &str| captures(r"`([^`]+)`", text).len();
    let ru: serde_json::Value = serde_json::from_str(hl1_guide::RU_JSON).unwrap();
    let mut table: Vec<&str> = ru["scripts"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    table.sort_unstable();
    let mut ids: Vec<&str> = hl1_guide::scripts().iter().map(|s| s.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(table, ids, "перевод без лишних и пропущенных скриптов");
    for script in hl1_guide::scripts() {
        let item = hl1_guide::translation_of(script, "ru")
            .unwrap_or_else(|| panic!("{}: перевод устарел после импорта гайда", script.id));
        let en = script
            .notes
            .iter()
            .chain(std::iter::once(&script.usage))
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        let text = item
            .notes
            .iter()
            .chain(std::iter::once(&item.usage))
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(links(&text), links(&en), "{}: ссылки", script.id);
        assert_eq!(code(&text), code(&en), "{}: код", script.id);
    }
    // Изменённый оригинал показывается без старого перевода.
    let mut changed = hl1_guide::scripts()[0].clone();
    changed.usage = "new text".into();
    assert_eq!(hl1_guide::translation_of(&changed, "ru"), None);
    assert_eq!(
        hl1_guide::translation_of(&hl1_guide::scripts()[0], "en"),
        None
    );
}

#[test]
fn ghosting_save_deletion_under_linux_goes_through_save_slash() {
    for (platform, expected) in [
        (Platform::Linux, "save /; reload"),
        (Platform::Win32, "save :; reload"),
    ] {
        assert_eq!(bind_presets::ghosting_save_deletion(platform), expected);
        let catalog = bind_presets::catalog_for("ghosting-renewed", platform).unwrap();
        assert!(
            catalog.preset_of(expected).is_some(),
            "заготовка удаления сейва"
        );
        assert!(
            catalog.preset_of("load quick01").is_some(),
            "бинды стандартного autoexec есть в заготовках"
        );
    }
}

#[test]
fn composite_binds_expose_their_scripts() {
    let parts = game_binds::commands_of("save oarEnt; bind mouse4 +tau; say TAO");
    assert_eq!(
        parts,
        ["save oarEnt", "bind mouse4 +tau", "+tau", "say TAO"]
    );
}

#[test]
fn script_behind_a_player_alias_chain_is_found_by_the_rebind_steps() {
    let chain = aliases(&[
        ("re4", "re41"),
        ("re41", "bind f fps4; say 4fps; alias re4 re42"),
        ("re42", "bind f obbo1000; say obbo1000; alias re4 re41"),
        ("obbo1000", "+use;w 8;-use;+jump;wait;-jump"),
        ("+hold", "fps_max 20"),
        ("-hold", "fps_max 100"),
    ]);
    let map = game_binds::alias_map(&chain);
    let parts = game_binds::expand_command("re4", &map);
    assert!(parts.contains(&"bind f obbo1000".to_owned()));
    assert!(parts.contains(&"obbo1000".to_owned()));
    assert!(
        game_binds::expand_command("+HOLD", &map).contains(&"fps_max 100".to_owned()),
        "у +x раскрывается и -x, регистр не важен"
    );
    let looped = game_binds::alias_map(&aliases(&[("a", "b"), ("b", "a")]));
    assert_eq!(
        game_binds::expand_command("a", &looped),
        ["a", "b", "a"],
        "цикл alias не зацикливает разбор"
    );
}

#[test]
fn in_scriptless_a_single_word_is_a_script_when_a_script_hides_behind_it() {
    let hl1 = bind_presets::catalog_for("hl1-won", Platform::Linux);
    let player = AliasInfo::new(&aliases(&[
        ("fps4", "fps_max 4; w; fps_max 100"),
        ("fps666", "fps_max 66.66666"),
        ("fix", "-jump;-duck;fps_max 100"),
        ("re5", "re51"),
        ("re51", "bind tab fps666; alias re5 re52"),
        ("re52", "bind tab fps4; alias re5 re51"),
        ("stepper", "fps666"),
        ("+myduck", "+duck"),
        ("-myduck", "-duck"),
        ("_saveload2", "w 27; save quick; load quick"),
    ]));
    let scripted = |command: &str| bind_presets::is_scripted(hl1, command, &player);
    assert!(scripted("fps4"), "alias с несколькими командами");
    assert!(scripted("FIX"), "имя alias без учёта регистра");
    assert!(
        scripted("re5"),
        "переключатель, который переопределяют другие alias"
    );
    assert!(
        scripted("+myduck"),
        "alias на +команду в Scriptless запрещён"
    );
    assert!(
        scripted("alias auto_alias _saveload2"),
        "alias, который запустит скрипт"
    );
    assert!(scripted("bind f fps4"), "перебинд на скрипт");
    assert!(scripted("bxt_autojump 1"), "bxt_autojump — только Scripted");
    assert!(!scripted("fps666"), "alias одной команды разрешён");
    assert!(
        !scripted("stepper"),
        "цепочка alias одной команды разрешена"
    );
    assert!(
        !bind_presets::is_scripted(hl1, "fps4", &AliasInfo::default()),
        "без alias игрока одно слово — не скрипт"
    );
}

#[test]
fn guide_scripts_on_the_keyboard_are_labelled_like_in_the_guide_in_english() {
    let catalog = bind_presets::catalog_for("hl1-won", Platform::Linux).unwrap();
    for command in ["+usespam", "+tau", "obboshoot", "4fpsbind"] {
        let preset = catalog.preset_of(command).unwrap();
        assert_eq!(preset.label.ru, preset.label.en, "{command}");
        assert!(
            preset.guide.is_some(),
            "{command}: связан со скриптом гайда"
        );
    }
    assert_eq!(
        catalog.preset_of("+bxt_tas_jumpbug").unwrap().label.ru,
        "Джампбаг (BXT)",
        "общие команды BXT не переименованы"
    );
}

#[test]
fn import_bom_and_utf16_of_notepad_exec_of_game_files_mid_line() {
    let utf16 = |text: &str, little: bool| -> String {
        text.encode_utf16()
            .flat_map(|unit| {
                if little {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                }
            })
            .map(char::from)
            .collect()
    };
    assert_eq!(
        game_binds::cfg_text("\u{ef}\u{bb}\u{bf}bind a +jump"),
        "bind a +jump"
    );
    assert_eq!(
        game_binds::cfg_text(&format!("\u{ff}\u{fe}{}", utf16("bind a +jump\r\n", true))),
        "bind a +jump\r\n"
    );
    assert_eq!(
        game_binds::cfg_text(&utf16("alias x \"+use\"\r\n", true)),
        "alias x \"+use\"\r\n",
        "UTF-16 без BOM"
    );
    let big = format!("\u{fe}\u{ff}{}", utf16("say привет", false));
    let bytes: Vec<u8> = game_binds::cfg_text(&big)
        .chars()
        .map(|c| c as u32 as u8)
        .collect();
    assert_eq!(
        String::from_utf8(bytes).unwrap(),
        "say привет",
        "UTF-16 BE, кириллица — байтами UTF-8"
    );
    assert_eq!(
        game_binds::cfg_text("say \u{cf}\u{f0}\u{e8}\u{e2}\u{e5}\u{f2}"),
        "say \u{cf}\u{f0}\u{e8}\u{e2}\u{e5}\u{f2}",
        "cp1251 не трогается"
    );
    assert_eq!(
        game_binds::parse_cfg(&game_binds::import_file_text(
            "\u{ef}\u{bb}\u{bf}bind a +jump",
            false
        ))[0],
        ["bind", "a", "+jump"]
    );
    let text = game_binds::import_file_text(
        "echo a; exec \"config.cfg\" ; bind x \"exec config.cfg; say hi\" // c\r\nexec valve.rc\r\nexec hlsr-import-x.cfg\r\n",
        false,
    );
    assert_eq!(
        text,
        "echo a; bind x \"exec config.cfg; say hi\"  // HLSR: exec \"config.cfg\" // c\r\n// HLSR: exec valve.rc\r\nexec hlsr-import-x.cfg\r\n"
    );
}

#[test]
fn import_launcher_overrides_are_lifted_only_from_keys_of_the_config() {
    let keys = game_binds::import_touched_keys(
        "unbindall\nbind \"MOUSE4\" \"+tau\"\nunbind f\nalias x \"bind k +use\"\n",
    );
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        ["MOUSE4", "f"],
        "unbindall и бинды внутри alias не в счёт"
    );
}
