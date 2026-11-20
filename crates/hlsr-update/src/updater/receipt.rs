//! Квитанция обновления (`updater/core/receipt.js`): после установки апдейтер пишет в
//! `version-state.json` `receipt {from, to, ts}`, по нему новая версия при запуске показывает
//! «лаунчер обновлён». Писатель файла один — `VersionEvents` подпроекта 4 (`hlsr-online`,
//! решение 15 спецификации): компактный JSON через `.tmp`, `seen` сохраняется. Здесь только порт;
//! реализацию подставляет приложение (решение координатора № 1). Формат «два пробела» апдейтера
//! 0.4.7 больше не производится — `JSON.parse` 0.4.7 читает оба.

use std::sync::Mutex;

/// Запись квитанции в файл плана (`plan.receiptFile`).
pub trait ReceiptStore: Send + Sync {
    /// `writePendingReceipt(file, fromVersion, toVersion)`: `true` — записана.
    fn write_pending_receipt(&self, file: &str, from: Option<&str>, to: &str) -> bool;
}

/// Подделка: записывает вызовы, отвечает заданным.
#[derive(Debug)]
pub struct FakeReceiptStore {
    result: bool,
    calls: Mutex<Vec<(String, Option<String>, String)>>,
}

impl FakeReceiptStore {
    pub fn new(result: bool) -> FakeReceiptStore {
        FakeReceiptStore {
            result,
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn calls(&self) -> Vec<(String, Option<String>, String)> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl ReceiptStore for FakeReceiptStore {
    fn write_pending_receipt(&self, file: &str, from: Option<&str>, to: &str) -> bool {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((file.to_owned(), from.map(str::to_owned), to.to_owned()));
        self.result
    }
}
