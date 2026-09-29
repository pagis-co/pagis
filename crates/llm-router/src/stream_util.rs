//! Stream adapters used by the router.

use std::time::Duration;

use async_stream::stream;
use futures::StreamExt;

use crate::error::Error;
use crate::protocol::EventStream;

/// End the stream with an in-band error when no event arrives within `idle`.
pub(crate) fn with_idle_timeout(
    events: EventStream,
    idle: Duration,
    provider: String,
) -> EventStream {
    stream! {
        let mut events = events;
        loop {
            match tokio::time::timeout(idle, events.next()).await {
                Ok(Some(item)) => yield item,
                Ok(None) => return,
                Err(_) => {
                    yield Err(Error::Stream {
                        provider,
                        message: format!("no stream event for {idle:?} (idle timeout)"),
                    });
                    return;
                }
            }
        }
    }
    .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::StreamEvent;
    use futures::stream;

    #[tokio::test(start_paused = true)]
    async fn ends_stream_with_error_when_idle() {
        let inner = stream::once(async { Ok(StreamEvent::TextDelta { text: "hi".into() }) })
            .chain(stream::pending())
            .boxed();
        let events: Vec<_> = with_idle_timeout(inner, Duration::from_millis(50), "p".into())
            .collect()
            .await;

        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0],
            Ok(StreamEvent::TextDelta { ref text }) if text == "hi"
        ));
        assert!(matches!(
            events[1],
            Err(Error::Stream { ref provider, .. }) if provider == "p"
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn passes_a_finished_stream_through() {
        let inner =
            stream::once(async { Ok(StreamEvent::TextDelta { text: "hi".into() }) }).boxed();
        let events: Vec<_> = with_idle_timeout(inner, Duration::from_millis(50), "p".into())
            .collect()
            .await;
        assert_eq!(events.len(), 1);
        assert!(events[0].is_ok());
    }
}
