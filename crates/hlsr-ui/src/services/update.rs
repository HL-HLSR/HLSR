//! Обновление лаунчера и диагностика (подпроект 6): каналы `UpdateApi::call` (карта §4.1:
//! `boot-gate-run`, `updater-*`, `diagnostics-run/cancel`, `export-diagnostics-zip`). Отказов у
//! этих каналов нет — ошибки приходят в ответе, как в 0.4.7.

use super::{BoxFuture, ChannelService, Reply};
use hlsr_update::api::UpdateApi;
use serde_json::Value;

impl ChannelService for UpdateApi {
    fn call<'a>(&'a self, channel: &'a str, args: &'a [Value]) -> BoxFuture<'a, Option<Reply>> {
        Box::pin(UpdateApi::call(self, channel, args))
    }
}
