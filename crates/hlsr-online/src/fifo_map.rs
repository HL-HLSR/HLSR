//! `Map` JS как кеш с вытеснением самой старой записи (`remember` в `SevenTvService` и
//! `TwitchEmoteService`): порядок вставки, `set` существующего ключа место не меняет.

/// Кеш: пары в порядке вставки.
#[derive(Debug, Clone)]
pub(crate) struct FifoMap<V> {
    entries: Vec<(String, V)>,
}

impl<V> Default for FifoMap<V> {
    fn default() -> Self {
        FifoMap {
            entries: Vec::new(),
        }
    }
}

impl<V> FifoMap<V> {
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }

    pub(crate) fn get(&self, key: &str) -> Option<&V> {
        self.entries
            .iter()
            .find(|(existing, _)| existing == key)
            .map(|(_, value)| value)
    }

    /// `remember(key, value)`: при `capacity` записях сначала уходит самая старая.
    pub(crate) fn remember(&mut self, key: &str, value: V, capacity: usize) {
        if self.entries.len() >= capacity && !self.entries.is_empty() {
            self.entries.remove(0);
        }
        match self
            .entries
            .iter_mut()
            .find(|(existing, _)| existing == key)
        {
            Some(entry) => entry.1 = value,
            None => self.entries.push((key.to_owned(), value)),
        }
    }
}

/// `Set` JS в порядке вставки с усечением до последних записей (`seen` и `sentEmoji` YouTube).
#[derive(Debug, Clone, Default)]
pub(crate) struct InsertionSet {
    order: Vec<String>,
    members: std::collections::HashSet<String>,
}

impl InsertionSet {
    pub(crate) fn len(&self) -> usize {
        self.order.len()
    }

    pub(crate) fn contains(&self, value: &str) -> bool {
        self.members.contains(value)
    }

    /// `set.add(value)`: у существующей записи место прежнее.
    pub(crate) fn insert(&mut self, value: &str) {
        if self.members.insert(value.to_owned()) {
            self.order.push(value.to_owned());
        }
    }

    pub(crate) fn clear(&mut self) {
        self.order.clear();
        self.members.clear();
    }

    /// `new Set(Array.from(set).slice(-count))`.
    pub(crate) fn keep_last(&mut self, count: usize) {
        if self.order.len() > count {
            let drop = self.order.len() - count;
            for value in self.order.drain(..drop) {
                self.members.remove(&value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FifoMap;

    #[test]
    fn evicts_the_oldest_at_capacity() {
        let mut map = FifoMap::default();
        for i in 0..256 {
            map.remember(&format!("k{i}"), i, 256);
        }
        assert_eq!(map.len(), 256);
        map.remember("new", 1000, 256);
        assert_eq!(map.len(), 256);
        assert!(map.get("k0").is_none());
        assert_eq!(map.get("k1"), Some(&1));
        assert_eq!(map.get("new"), Some(&1000));
    }

    #[test]
    fn existing_key_keeps_its_place_but_still_evicts_when_full() {
        let mut map = FifoMap::default();
        for i in 0..3 {
            map.remember(&format!("k{i}"), i, 3);
        }
        // `Map.set` существующего ключа место не меняет, но `remember` сначала вытесняет самый старый.
        map.remember("k1", 10, 3);
        assert!(map.get("k0").is_none());
        assert_eq!(map.get("k1"), Some(&10));
        assert_eq!(map.len(), 2);
        map.remember("k3", 3, 3);
        map.remember("k4", 4, 3);
        assert!(map.get("k1").is_none(), "k1 была самой старой");
        assert_eq!(map.len(), 3);
    }

    #[test]
    fn insertion_set_keeps_the_last_entries() {
        let mut set = super::InsertionSet::default();
        for i in 0..10 {
            set.insert(&format!("v{i}"));
        }
        set.insert("v3");
        assert_eq!(set.len(), 10);
        set.keep_last(4);
        assert_eq!(set.len(), 4);
        assert!(!set.contains("v5"));
        assert!(set.contains("v6") && set.contains("v9"));
        set.clear();
        assert_eq!(set.len(), 0);
    }

    #[test]
    fn clear_empties_the_map() {
        let mut map = FifoMap::default();
        map.remember("a", 1, 4);
        map.clear();
        assert_eq!(map.len(), 0);
        assert!(map.get("a").is_none());
    }
}
