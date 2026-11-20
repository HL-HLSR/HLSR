//! Платформа (подпроект 3): каналы `PlatformApi::call` (запуск, процессы, Steam, Proton, трей).

use super::{BoxFuture, ChannelService, Reply};
use hlsr_platform::api::PlatformApi;
use serde_json::Value;

impl ChannelService for PlatformApi {
    fn call<'a>(&'a self, channel: &'a str, args: &'a [Value]) -> BoxFuture<'a, Option<Reply>> {
        Box::pin(PlatformApi::call(self, channel, args))
    }
}
