//! Онлайн (подпроект 4): каналы `OnlineApi::call`.

use super::{BoxFuture, ChannelService, Reply};
use hlsr_online::api::OnlineApi;
use serde_json::Value;

impl ChannelService for OnlineApi {
    fn call<'a>(&'a self, channel: &'a str, args: &'a [Value]) -> BoxFuture<'a, Option<Reply>> {
        Box::pin(OnlineApi::call(self, channel, args))
    }
}
