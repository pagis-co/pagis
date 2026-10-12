//! The host transport of a Notification.

use async_trait::async_trait;
use pagis_push::{Options, Outcome, Subscription};

/// Send a Web Push. A wasm host builds it with `WebPush::request`, posts it
/// under that method's endpoint guard and timeout rules, and reads `Outcome::of`.
#[async_trait]
pub trait NotificationSender: Send + Sync {
    async fn send(
        &self,
        subscription: &Subscription,
        plaintext: &[u8],
        options: Options,
    ) -> Outcome;
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
impl NotificationSender for pagis_push::WebPush {
    async fn send(
        &self,
        subscription: &Subscription,
        plaintext: &[u8],
        options: Options,
    ) -> Outcome {
        pagis_push::WebPush::send(self, subscription, plaintext, options).await
    }
}
