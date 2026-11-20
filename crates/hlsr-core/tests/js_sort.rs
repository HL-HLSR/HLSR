//! `Array.prototype.sort` с функцией сравнения против Node 24 (V8 15.2 Electron 0.4.7). Фикстура
//! `node-semantics-sort.json` из `tools/reference/node-semantics-sort.js`: элемент — ключ (число или
//! `null` вместо NaN), сравнение `a.k - b.k`, результат — номера элементов после сортировки. До 15
//! элементов V8 ставит их двоичными вставками с первого, 16–63 — находит серию от начала и дополняет
//! вставками; порядок воспроизводится даже при NaN (сравнение с ним — ноль, непостоянное). От 64
//! V8 сливает серии: сверяются только постоянные сравнения с ничьими.

use hlsr_core::js::sort::sort_by_comparator;
use serde_json::Value;

fn fixture() -> Value {
    let file = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/node-semantics-sort.json"
    );
    serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
}

fn keys_of(case: &Value) -> Vec<f64> {
    case["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|key| key.as_f64().unwrap_or(f64::NAN))
        .collect()
}

#[test]
fn sort_matches_node_for_every_case() {
    let fixture = fixture();
    for case in fixture["cases"].as_array().unwrap() {
        let keys = keys_of(case);
        let expected: Vec<usize> = case["order"]
            .as_array()
            .unwrap()
            .iter()
            .map(|index| index.as_u64().unwrap() as usize)
            .collect();
        let mut items: Vec<usize> = (0..keys.len()).collect();
        sort_by_comparator(&mut items, |&a, &b| keys[a] - keys[b]);
        assert_eq!(items, expected, "ключи {:?}", case["keys"]);
    }
}

/// Фикстура не должна вырождаться: массивов с NaN в каждом классе длин по меньшей мере `minPerClass`
/// (раньше генератор брал младшие биты LCG, и таких было двенадцать, все короче восьми), у каждой
/// длины 2–63 есть случаи с NaN, и сам алгоритм V8 даёт на них не тот порядок, что устойчивая
/// сортировка по ключам без NaN.
#[test]
fn the_corpus_keeps_nan_cases_in_every_length_class() {
    let fixture = fixture();
    let minimum = fixture["meta"]["minPerClass"].as_u64().unwrap() as usize;
    assert!(minimum >= 200, "порог в фикстуре занижен: {minimum}");
    assert_eq!(
        fixture["meta"]["jitless"], true,
        "эталон снят без --jitless"
    );
    assert!(
        fixture["meta"]["v8"].as_str().unwrap().starts_with("15.2"),
        "эталон снят не в V8 15.2 Electron 0.4.7: {}",
        fixture["meta"]["v8"]
    );

    let (mut short, mut medium, mut long) = (0, 0, 0);
    let mut nan_lengths = std::collections::BTreeSet::new();
    let mut inverted = 0;
    for case in fixture["cases"].as_array().unwrap() {
        let keys = keys_of(case);
        let has_nan = keys.iter().any(|key| key.is_nan());
        match keys.len() {
            2..=15 if has_nan => short += 1,
            16..=63 if has_nan => medium += 1,
            64..=200 if !has_nan => long += 1,
            _ => {}
        }
        assert!(
            !(has_nan && keys.len() >= 64),
            "NaN в массиве от 64 элементов не сверяется"
        );
        if has_nan {
            nan_lengths.insert(keys.len());
            // Порядок V8 при NaN нарушает порядок остальных ключей (при постоянном сравнении так не бывает).
            let order: Vec<usize> = case["order"]
                .as_array()
                .unwrap()
                .iter()
                .map(|index| index.as_u64().unwrap() as usize)
                .collect();
            let sorted_keys: Vec<f64> = order
                .iter()
                .map(|&index| keys[index])
                .filter(|key| !key.is_nan())
                .collect();
            if sorted_keys.windows(2).any(|pair| pair[0] > pair[1]) {
                inverted += 1;
            }
        }
    }
    assert!(short >= minimum, "короткие массивы с NaN: {short}");
    assert!(medium >= minimum, "массивы 16–63 с NaN: {medium}");
    assert!(long >= minimum, "длинные массивы с ничьими: {long}");
    assert!(
        (2..64).all(|length| nan_lengths.contains(&length)),
        "не у каждой длины 2–63 есть случай с NaN: {nan_lengths:?}"
    );
    assert!(
        inverted >= minimum,
        "случаев, где NaN нарушает порядок остальных ключей, мало: {inverted}"
    );
}

#[test]
fn sort_is_stable_and_never_panics_on_a_comparator_that_is_not_an_order() {
    // Сравнение, у которого «не больше» — не порядок: Rust sort_by вправе здесь паниковать.
    let mut items: Vec<u32> = (0..500).collect();
    sort_by_comparator(&mut items, |&a, &b| {
        if (a ^ b) % 3 == 0 {
            0.0
        } else {
            f64::from(a) - f64::from(b)
        }
    });
    let mut sorted = items.clone();
    sorted.sort_unstable();
    assert_eq!(
        sorted,
        (0..500).collect::<Vec<_>>(),
        "ни один элемент не потерян"
    );

    let mut pairs: Vec<(u32, u32)> = (0..300).map(|n| (n % 7, n)).collect();
    sort_by_comparator(&mut pairs, |a, b| f64::from(a.0) - f64::from(b.0));
    assert!(
        pairs
            .windows(2)
            .all(|w| w[0].0 < w[1].0 || (w[0].0 == w[1].0 && w[0].1 < w[1].1))
    );
}
