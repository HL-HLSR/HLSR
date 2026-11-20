//! Доставка (подпроект 2): каналы `DeliveryApi::call`. Отказов у каналов доставки нет — ошибки
//! приходят в ответе (`{status: "error", code}`).

use super::{BoxFuture, ChannelService, Reply};
use hlsr_delivery::api::DeliveryApi;
use serde_json::Value;

impl ChannelService for DeliveryApi {
    fn call<'a>(&'a self, channel: &'a str, args: &'a [Value]) -> BoxFuture<'a, Option<Reply>> {
        Box::pin(async move { DeliveryApi::call(self, channel, args).await.map(Ok) })
    }
}
