//! Свободное место с кешем на 5 секунд (`free-space.js`).

use crate::ports::FreeSpace;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// `CACHE_MS`.
pub const CACHE_MS: u64 = 5000;

/// `freeSpaceBytes` с общим кешем: ответ (и неудача тоже) помнится 5 с по строке пути.
pub struct FreeSpaceCache {
    source: Arc<dyn FreeSpace>,
    entries: Mutex<HashMap<String, (Instant, Option<u64>)>>,
}

impl FreeSpaceCache {
    pub fn new(source: Arc<dyn FreeSpace>) -> FreeSpaceCache {
        FreeSpaceCache {
            source,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// `freeSpaceBytes(dir)`.
    pub fn free_space_bytes(&self, dir: &Path) -> Option<u64> {
        self.free_space_bytes_at(dir, Instant::now())
    }

    /// `freeSpaceBytes(dir, { now })`.
    pub fn free_space_bytes_at(&self, dir: &Path, now: Instant) -> Option<u64> {
        let key = dir.to_string_lossy().into_owned();
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(&(at, value)) = entries.get(&key)
            && now.saturating_duration_since(at) < Duration::from_millis(CACHE_MS)
        {
            return value;
        }
        let value = self.source.free_bytes(dir);
        entries.insert(key, (now, value));
        value
    }

    /// `forget()`: сбросить кеш.
    pub fn forget(&self) {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }
}

/// Кеш сам отвечает как источник места: его можно отдать сервису вместо `freeSpaceBytes`.
impl FreeSpace for FreeSpaceCache {
    fn free_bytes(&self, dir: &Path) -> Option<u64> {
        self.free_space_bytes(dir)
    }
}
