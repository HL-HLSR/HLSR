//! Простой объект JS как словарь со строковыми ключами.
//!
//! `Object.keys`, `Object.entries` и `for…in` отдают ключи не в порядке
//! вставки: сначала «индексы массива» (`"0"`, `"7"`, `"130"` — каноническая
//! запись целого от 0 до 2³² − 2) по возрастанию, потом остальные ключи в
//! порядке вставки. Присвоение существующему ключу место не меняет, `delete`
//! убирает ключ, а новая вставка идёт в конец. От этого порядка зависят
//! обрезка по числу записей, порядок строк в cfg и байты JSON.

use crate::json_text::array_index;
use std::collections::HashMap;

/// Объект JS (`{}` или `Object.create(null)`): значения по строковым ключам.
#[derive(Debug, Clone)]
pub struct JsObject<V> {
    /// Записи в порядке вставки.
    entries: Vec<(String, V)>,
    /// Место ключа в `entries`.
    positions: HashMap<String, usize>,
}

impl<V> Default for JsObject<V> {
    fn default() -> Self {
        JsObject {
            entries: Vec::new(),
            positions: HashMap::new(),
        }
    }
}

impl<V> JsObject<V> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.positions.contains_key(key)
    }

    pub fn get(&self, key: &str) -> Option<&V> {
        self.positions.get(key).map(|&at| &self.entries[at].1)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut V> {
        let at = *self.positions.get(key)?;
        Some(&mut self.entries[at].1)
    }

    /// `object[key] = value`: у существующего ключа место прежнее.
    /// Возвращает прежнее значение.
    pub fn insert(&mut self, key: impl Into<String>, value: V) -> Option<V> {
        let key = key.into();
        match self.positions.get(&key) {
            Some(&at) => Some(std::mem::replace(&mut self.entries[at].1, value)),
            None => {
                self.positions.insert(key.clone(), self.entries.len());
                self.entries.push((key, value));
                None
            }
        }
    }

    /// `delete object[key]`.
    pub fn remove(&mut self, key: &str) -> Option<V> {
        let at = self.positions.remove(key)?;
        let (_, value) = self.entries.remove(at);
        for (moved, _) in &self.entries[at..] {
            if let Some(position) = self.positions.get_mut(moved) {
                *position -= 1;
            }
        }
        Some(value)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.positions.clear();
    }

    /// Записи в порядке `Object.entries`.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &V)> {
        self.order()
            .into_iter()
            .map(|at| (self.entries[at].0.as_str(), &self.entries[at].1))
    }

    /// Ключи в порядке `Object.keys`.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.iter().map(|(key, _)| key)
    }

    /// Значения в порядке `Object.values`.
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.iter().map(|(_, value)| value)
    }

    /// Места записей в `entries` в порядке ключей JS.
    fn order(&self) -> Vec<usize> {
        let mut indices: Vec<(u32, usize)> = Vec::new();
        let mut others = Vec::new();
        for (at, (key, _)) in self.entries.iter().enumerate() {
            match array_index(key) {
                Some(index) => indices.push((index, at)),
                None => others.push(at),
            }
        }
        if indices.is_empty() {
            return others;
        }
        indices.sort_unstable();
        indices
            .into_iter()
            .map(|(_, at)| at)
            .chain(others)
            .collect()
    }
}

impl<V: PartialEq> PartialEq for JsObject<V> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter())
    }
}

impl<V: Eq> Eq for JsObject<V> {}

impl<K: Into<String>, V> FromIterator<(K, V)> for JsObject<V> {
    fn from_iter<T: IntoIterator<Item = (K, V)>>(iter: T) -> Self {
        let mut object = JsObject::new();
        for (key, value) in iter {
            object.insert(key, value);
        }
        object
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(object: &JsObject<i32>) -> Vec<&str> {
        object.keys().collect()
    }

    #[test]
    fn array_index_keys_come_first_in_ascending_order() {
        let mut object = JsObject::new();
        for key in ["b", "2", "a", "1", "10", "01", "4294967294", "4294967295"] {
            object.insert(key, 0);
        }
        assert_eq!(
            keys(&object),
            ["1", "2", "10", "4294967294", "b", "a", "01", "4294967295"]
        );
    }

    #[test]
    fn assignment_keeps_position_and_delete_moves_to_the_end() {
        let mut object = JsObject::new();
        object.insert("x", 1);
        object.insert("y", 2);
        assert_eq!(object.insert("x", 3), Some(1));
        assert_eq!(keys(&object), ["x", "y"]);
        assert_eq!(object.remove("x"), Some(3));
        assert_eq!(object.remove("x"), None);
        object.insert("x", 4);
        assert_eq!(keys(&object), ["y", "x"]);
        assert_eq!(object.get("x"), Some(&4));
        assert_eq!(object.get("y"), Some(&2));
    }

    #[test]
    fn equality_follows_key_order_of_js() {
        let a: JsObject<i32> = [("b", 1), ("a", 2)].into_iter().collect();
        let b: JsObject<i32> = [("a", 2), ("b", 1)].into_iter().collect();
        assert_ne!(a, b);
        let c: JsObject<i32> = [("2", 1), ("1", 2)].into_iter().collect();
        let d: JsObject<i32> = [("1", 2), ("2", 1)].into_iter().collect();
        assert_eq!(c, d);
    }
}
