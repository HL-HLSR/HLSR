//! Чистые функции источника HL1: сравнение отпечатков (`sameStamp`), отпечаток встроенного архива
//! (`ORIGINS`). `versionFor` 0.4.9 убрал: ставится только закреплённая сборка.

use super::scenario::{FIXTURE, check, ok};
use super::support::{Mismatches, items};
use hlsr_delivery::manifests;
use hlsr_delivery::origin_updates::{default_origins, same_stamp};
use serde_json::{Value, json};

#[test]
fn default_origins_match_0_4_7() {
    let origins = default_origins();
    let actual: Value = origins
        .iter()
        .map(|(game_id, origin)| {
            (
                game_id.clone(),
                json!({
                    "providerId": origin.provider_id,
                    "baseline": {
                        "version": origin.baseline.version,
                        "lastModified": origin.baseline.last_modified,
                        "etag": origin.baseline.etag,
                        "size": origin.baseline.size,
                    },
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>()
        .into();
    let mut m = Mismatches::default();
    check(&mut m, &actual, &FIXTURE["stamps"]["origins"], "ORIGINS");
    m.finish("ORIGINS");
}

/// Отпечаток соответствует встроенному архиву (`hl1-selftest.js`).
#[test]
fn baseline_size_is_the_size_of_the_bundled_archive() {
    let size = manifests::manifest_of("hl1-won").unwrap()["baseArchive"]["size"].clone();
    assert_eq!(size, FIXTURE["stamps"]["baselineSize"]);
    assert_eq!(json!(default_origins()[0].1.baseline.size), size);
}

#[test]
fn same_stamp_matches_0_4_7() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["stamps"]["sameStamp"]) {
        let actual = ok(json!(same_stamp(&case["a"], &case["b"])));
        check(
            &mut m,
            &actual,
            &case["result"],
            format!("sameStamp: {}", case["name"]),
        );
    }
    m.finish("sameStamp");
}
