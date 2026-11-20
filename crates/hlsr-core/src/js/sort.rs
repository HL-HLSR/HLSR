//! `Array.prototype.sort(compare)` как в V8 15.2 (Electron 0.4.7, Node 24): встроенная сортировка
//! `third_party/v8/builtins/array-sort.tq` — TimSort.
//!
//! Функция сравнения в JS может быть не порядком: `(a, b) => a.ts - b.ts` при нечисловом `ts` даёт
//! NaN, а NaN V8 считает нулём («равны»). Тогда порядок задаёт только сам алгоритм, а стандартная
//! сортировка Rust вправе паниковать. Алгоритм зависит от длины (сверено с Electron 0.4.7 на
//! совершенно произвольных, не симметричных сравнениях, в том числе по последовательности вызовов):
//!
//! - до 15 элементов — двоичные вставки с первого элемента, без поиска серии;
//! - 16–63 — серия от начала (неубывающая или строго убывающая, её переворачивают), остальное
//!   двоичными вставками;
//! - от 64 V8 сливает серии. Здесь вся оставшаяся часть добавляется теми же вставками: при сравнении,
//!   которое является порядком, итог тот же (сортировка устойчива), при непостоянном — нет.
//!
//! Что сверялось: встроенная сортировка, то есть вызов из неоптимизированного кода (так 0.4.7
//! сортирует версии и заметки: вызовы редкие). Вызов, который V8 уже скомпилировал (Maglev, затем
//! TurboFan; на Electron 44 — после ~1000 вызовов из одного места), для массивов до 16 элементов с непостоянным сравнением даёт
//! другой порядок; его эталон не повторяет. Node 26 (V8 14.6) тоже сортирует короткие массивы
//! иначе (с 8 элементов серия от начала), поэтому фикстура снимается только в Electron эталона.

/// `items.sort(compare)`: `compare` возвращает число, как функция сравнения JS; NaN — ноль.
pub fn sort_by_comparator<T>(items: &mut [T], mut compare: impl FnMut(&T, &T) -> f64) {
    /// С такой длины V8 ищет серию от начала (меньше — сразу двоичные вставки).
    const RUN_DETECTION_FROM: usize = 16;

    let length = items.len();
    if length < 2 {
        return;
    }
    let mut order = |a: &T, b: &T| {
        let value = compare(a, b);
        if value.is_nan() { 0.0 } else { value }
    };

    // CountAndMakeRun: серия от начала; убывающая только строго.
    let mut run = 1;
    if length >= RUN_DETECTION_FROM {
        let descending = order(&items[1], &items[0]) < 0.0;
        run = 2;
        let mut previous = 1;
        for current in 2..length {
            let value = order(&items[current], &items[previous]);
            if if descending {
                value >= 0.0
            } else {
                value < 0.0
            } {
                break;
            }
            previous = current;
            run += 1;
        }
        if descending {
            items[..run].reverse();
        }
    }

    // BinaryInsertionSort: место элемента — после всех, которые не больше него.
    for start in run..length {
        let (mut left, mut right) = (0, start);
        while left < right {
            let middle = left + ((right - left) >> 1);
            if order(&items[start], &items[middle]) < 0.0 {
                right = middle;
            } else {
                left = middle + 1;
            }
        }
        items[left..=start].rotate_right(1);
    }
}
