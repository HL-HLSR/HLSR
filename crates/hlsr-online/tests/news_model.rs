//! Модель ленты и спонсоров против 0.4.7: фикстура `news-model.json` из
//! `tools/reference/online/news-model.js` — чистые функции `news-client.js`, `sponsors-client.js`,
//! `news-ipc.js` (`externalUrl`), `news-contract.js`, `news-image-layout.js` и `news-locales.js`
//! в Node 24 на корпусе входов. Самотесты 0.4.7 (`scripts/news-selftest.js`) входят в корпус.
//! Лента 0.4.9 (свежие забеги, места, профили бегунов, `subscriber`, `parseProfileCard`, строки) —
//! фикстура `news-model-049.json` из `news-model-049.js`: корпус 0.4.7 плюс случаи 0.4.9.
//!
//! Корпус — то, что приходит из `JSON.parse`; `{"undefined": true}` — поля нет.

mod support;

use hlsr_core::json_text::js_number;
use hlsr_online::shared::news_image_layout::{self, Crop, TileImage};
use hlsr_online::shared::{news_contract, news_locales};
use hlsr_online::telemetry::news_client::{parse_author_profile, parse_item, parse_profile_card};
use hlsr_online::telemetry::news_ipc::external_url;
use hlsr_online::telemetry::sponsors_client::parse_sponsor;
use serde_json::{Value, json};
use support::{Mismatches, fixture, items, str_of};

/// Вход случая: `None`, если в корпусе стоял `undefined`.
fn raw_of<'a>(case: &'a Value, key: &str) -> Option<&'a Value> {
    let value = &case[key];
    (value.get("undefined") != Some(&json!(true))).then_some(value)
}

fn crop_json(crop: Option<Crop>) -> Value {
    crop.map_or(Value::Null, |crop| crop.to_json())
}

/// Картинка плитки как её считал Node: размеры и область из корпуса, без `parseCrop`.
fn tile_image(image: &Value) -> TileImage {
    let part = |crop: &Value, key: &str| crop[key].as_f64().unwrap();
    TileImage {
        width: image["width"].as_f64().unwrap(),
        height: image["height"].as_f64().unwrap(),
        crop: image.get("crop").map(|crop| Crop {
            x: part(crop, "x"),
            y: part(crop, "y"),
            w: part(crop, "w"),
            h: part(crop, "h"),
        }),
    }
}

#[test]
fn reactions_table_matches_node() {
    let fixture = fixture("news-model.json");
    let table = fixture["reactions"].as_object().unwrap();
    assert_eq!(table.len(), news_contract::REACTIONS.len());
    for (key, emoji) in news_contract::REACTIONS {
        assert_eq!(str_of(&table[key]), emoji, "{key}");
    }
    let order: Vec<&str> = table.keys().map(String::as_str).collect();
    let ours: Vec<&str> = news_contract::REACTIONS
        .iter()
        .map(|(key, _)| *key)
        .collect();
    assert_eq!(order, ours);
}

#[test]
fn locales_match_node() {
    let fixture = fixture("news-model-049.json");
    for locale in ["ru", "en"] {
        let expected = fixture["locales"][locale].as_object().unwrap();
        let ours = news_locales::strings(locale).unwrap();
        assert_eq!(ours.len(), expected.len(), "{locale}: число строк");
        for (key, text) in expected {
            assert_eq!(
                news_locales::get(locale, key),
                Some(str_of(text)),
                "{locale} {key}"
            );
        }
        let order: Vec<&str> = expected.keys().map(String::as_str).collect();
        let names: Vec<&str> = ours.iter().map(|(key, _)| *key).collect();
        assert_eq!(names, order, "{locale}: порядок");
    }
    assert_eq!(news_locales::strings("de"), None);
}

#[test]
fn post_ids_counts_and_mine_match_node() {
    let fixture = fixture("news-model.json");
    let mut mismatches = Mismatches::default();
    for case in items(&fixture["postIds"]) {
        let valid = raw_of(case, "id")
            .and_then(Value::as_str)
            .is_some_and(news_contract::valid_post_id);
        mismatches.check(
            &json!(valid),
            &case["valid"],
            format!("validPostId({})", case["id"]),
        );
    }
    for case in items(&fixture["counts"]) {
        let counts = news_contract::counts(raw_of(case, "raw"));
        mismatches.check(
            &counts.to_json(),
            &case["counts"],
            format!("counts({})", case["raw"]),
        );
    }
    for case in items(&fixture["mine"]) {
        let mine = news_contract::mine(raw_of(case, "raw"));
        mismatches.check(
            &json!(mine),
            &case["mine"],
            format!("mine({})", case["raw"]),
        );
    }
    mismatches.finish("news-contract");
}

#[test]
fn crop_aspect_and_style_match_node() {
    let fixture = fixture("news-model.json");
    let mut mismatches = Mismatches::default();
    for case in items(&fixture["crops"]) {
        let crop = news_image_layout::parse_crop(raw_of(case, "raw"));
        mismatches.check(
            &crop_json(crop),
            &case["crop"],
            format!("parseCrop({})", case["raw"]),
        );
    }
    for case in items(&fixture["aspects"]) {
        let aspect = news_image_layout::tile_aspect(
            &tile_image(&case["image"]),
            case["index"].as_u64().unwrap() as usize,
            case["count"].as_u64().unwrap() as usize,
        );
        mismatches.check(
            &js_number(aspect),
            &case["aspect"],
            format!(
                "tileAspect({}, {}, {})",
                case["image"], case["index"], case["count"]
            ),
        );
    }
    for case in items(&fixture["styles"]) {
        let style = news_image_layout::tile_image_style(
            &tile_image(&case["image"]),
            case["aspect"].as_f64().unwrap(),
        );
        mismatches.check(
            &style.to_json(),
            &case["style"],
            format!("tileImageStyle({}, {})", case["image"], case["aspect"]),
        );
    }
    mismatches.finish("news-image-layout");
}

#[test]
fn author_profile_matches_node() {
    let fixture = fixture("news-model-049.json");
    let mut mismatches = Mismatches::default();
    for case in items(&fixture["authors"]) {
        let actual = parse_author_profile(raw_of(case, "raw"))
            .map_or(Value::Null, |profile| profile.to_json());
        mismatches.check(
            &actual,
            &case["profile"],
            format!("parseAuthorProfile({})", case["raw"]),
        );
    }
    mismatches.finish("parseAuthorProfile");
}

#[test]
fn profile_card_matches_node() {
    let fixture = fixture("news-model-049.json");
    let mut mismatches = Mismatches::default();
    for case in items(&fixture["cards"]) {
        let actual =
            parse_profile_card(raw_of(case, "raw")).map_or(Value::Null, |card| card.to_json());
        mismatches.check(
            &actual,
            &case["card"],
            format!("parseProfileCard({})", case["raw"]),
        );
    }
    mismatches.finish("parseProfileCard");
}

/// Результат `parseItem` или `null`; `{"undefined": true}` у Node не бывает.
fn item_json(raw: &Value, api_base: &str) -> Value {
    parse_item(raw, api_base).map_or(Value::Null, |item| item.to_json())
}

const API: &str = "https://api.hlsr.pro";

#[test]
fn parse_item_posts_match_node() {
    let fixture = fixture("news-model-049.json");
    let cases = items(&fixture["posts"]);
    assert!(cases.len() > 80);
    let mut mismatches = Mismatches::default();
    for case in cases {
        mismatches.check(
            &item_json(&case["raw"], API),
            &case["item"],
            format!("parseItem({})", case["raw"]),
        );
    }
    mismatches.finish("parseItem: посты");
}

#[test]
fn parse_item_records_match_node() {
    let fixture = fixture("news-model-049.json");
    let cases = items(&fixture["records"]);
    assert!(cases.len() > 120);
    let mut mismatches = Mismatches::default();
    for case in cases {
        mismatches.check(
            &item_json(&case["raw"], API),
            &case["item"],
            format!("parseItem({})", case["raw"]),
        );
    }
    mismatches.finish("parseItem: рекорды");
}

/// Адрес сервера сценария в корпусе — метка; Node считал на `127.0.0.1:18896`.
const AUTHORITY: &str = "127.0.0.1:18896";

#[test]
fn parse_item_images_match_node() {
    let fixture = fixture("news-model.json");
    let cases = items(&fixture["images"]);
    assert!(cases.len() > 120);
    let mut mismatches = Mismatches::default();
    for case in cases {
        let api_base = str_of(&case["apiBase"]);
        let base = match api_base.strip_prefix("{authority}") {
            Some(rest) => format!("http://{AUTHORITY}{rest}"),
            None => api_base.to_owned(),
        };
        let raw: Value =
            serde_json::from_str(&case["raw"].to_string().replace("{authority}", AUTHORITY))
                .unwrap();
        let actual = item_json(&raw, &base);
        // Адрес сервера в результате — снова метка.
        let actual: Value =
            serde_json::from_str(&actual.to_string().replace(AUTHORITY, "{authority}")).unwrap();
        mismatches.check(
            &actual,
            &case["item"],
            format!("parseItem(images) apiBase={api_base}: {}", case["raw"]),
        );
    }
    mismatches.finish("parseItem: картинки");
}

#[test]
fn parse_sponsor_matches_node() {
    let fixture = fixture("news-model.json");
    let mut mismatches = Mismatches::default();
    for case in items(&fixture["sponsors"]) {
        let mut actual = raw_of(case, "raw")
            .and_then(parse_sponsor)
            .map_or(Value::Null, |sponsor| sponsor.to_json());
        // `profile.subscriber` появился в 0.4.9 (O-3, `sponsors-049.json`); в корпусе 0.4.7 его нет;
        // `parseProfileCard` с ним сверяет `news-model-049.json`.
        if case["sponsor"]["profile"].get("subscriber").is_none()
            && let Some(profile) = actual.get_mut("profile").and_then(Value::as_object_mut)
        {
            profile.remove("subscriber");
        }
        mismatches.check(
            &actual,
            &case["sponsor"],
            format!("parseSponsor({})", case["raw"]),
        );
    }
    mismatches.finish("parseSponsor");
}

#[test]
fn external_url_matches_node() {
    let fixture = fixture("news-model.json");
    let mut mismatches = Mismatches::default();
    for case in items(&fixture["externalUrls"]) {
        let raw = str_of(&case["raw"]);
        let actual = match external_url(raw) {
            Ok(url) => json!({"ok": {"href": url.as_str(), "hostname": url.host_str()}}),
            Err(_) => json!({"error": true}),
        };
        let mut expected = case.clone();
        expected.as_object_mut().unwrap().remove("raw");
        mismatches.check(&actual, &expected, format!("externalUrl({raw:?})"));
    }
    mismatches.finish("externalUrl");
}

/// Случаи самотестов 0.4.7 (`news-selftest.js:147-171`, `:317-330`) с их прямыми проверками:
/// корпус выше сверяет те же входы с Node, здесь — ожидания самих тестов.
#[test]
fn selftest_layout_and_record_links() {
    let wide = TileImage {
        width: 1600.0,
        height: 900.0,
        crop: None,
    };
    let with = |x, y, w, h| TileImage {
        crop: Some(Crop { x, y, w, h }),
        ..wide
    };
    // Один скриншот: пропорции области, узкая упирается в 4:3, сверхширокая — в 21:9.
    assert_eq!(news_image_layout::tile_aspect(&wide, 0, 1), 16.0 / 9.0);
    assert_eq!(
        news_image_layout::tile_aspect(&with(0.0, 0.0, 0.3, 1.0), 0, 1),
        4.0 / 3.0
    );
    assert_eq!(
        news_image_layout::tile_aspect(&with(0.0, 0.0, 1.0, 0.2), 0, 1),
        21.0 / 9.0
    );
    // Несколько: нечётное число — первый 2:1, остальные 16:9.
    assert_eq!(news_image_layout::tile_aspect(&wide, 0, 3), 2.0);
    assert_eq!(news_image_layout::tile_aspect(&wide, 1, 3), 16.0 / 9.0);
    assert_eq!(news_image_layout::tile_aspect(&wide, 0, 2), 16.0 / 9.0);
    let style = news_image_layout::tile_image_style(&wide, 16.0 / 9.0);
    assert_eq!(
        (
            style.width.as_str(),
            style.height.as_str(),
            style.left.as_str(),
            style.top.as_str()
        ),
        ("100%", "100%", "0%", "0%")
    );
    // Правая половина: картинка вдвое шире плитки и сдвинута на её ширину влево.
    let style = news_image_layout::tile_image_style(&with(0.5, 0.0, 0.5, 1.0), 8.0 / 9.0);
    assert_eq!(
        (
            style.width.as_str(),
            style.height.as_str(),
            style.left.as_str(),
            style.top.as_str()
        ),
        ("200%", "100%", "-100%", "0%")
    );

    // Ссылка рекорда ведёт на страницу забега с игрой; обе формы weblink принимаются.
    let record = |extra: Value| {
        let mut base = json!({
            "kind": "record", "id": "z16q06rm", "game": "Half-Life 2", "gameKey": "hl2",
            "category": "New Engine", "runners": ["Buster12"], "seconds": 2190.285,
            "date": "2025-07-15T22:14:46+00:00",
            "url": "https://www.speedrun.com/runs/z16q06rm",
        });
        base.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        base
    };
    let url_of = |raw: Value| item_json(&raw, API)["url"].as_str().map(str::to_owned);
    assert_eq!(
        url_of(record(json!({}))).as_deref(),
        Some("https://www.speedrun.com/hl2/runs/z16q06rm")
    );
    assert_eq!(
        url_of(record(
            json!({"url": "https://www.speedrun.com/hl2/runs/y24vkp5z"})
        ))
        .as_deref(),
        Some("https://www.speedrun.com/hl2/runs/y24vkp5z")
    );
    assert_eq!(
        url_of(record(
            json!({"gameKey": "", "url": "https://www.speedrun.com/runs/ylw19jnz"})
        ))
        .as_deref(),
        Some("https://www.speedrun.com/runs/ylw19jnz")
    );
    let weblink = "https://www.speedrun.com/hl2/run/z16q06rm";
    assert_eq!(
        url_of(record(json!({"url": weblink}))).as_deref(),
        Some(weblink)
    );
}
