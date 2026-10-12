//! A host's cancellation and message delivery for running Runs.

use pagis_core::{Message, RunId};

pub trait RunSignals {
    /// Signal cancellation of the named Run. Return false if it is absent.
    fn cancel(&self, run_id: &RunId) -> bool;

    /// Give a message to the named Run. Return an unsent message when the
    /// Run is absent or has stopped reading, so the caller can queue it.
    fn inject(&self, run_id: &RunId, message: Message) -> Option<Message>;
}

#[cfg(test)]
mod tests {
    use super::RunSignals;

    #[test]
    fn another_host_implements_run_signals() {
        struct Signals;
        impl RunSignals for Signals {
            fn cancel(&self, _run_id: &pagis_core::RunId) -> bool {
                false
            }

            fn inject(
                &self,
                _run_id: &pagis_core::RunId,
                message: pagis_core::Message,
            ) -> Option<pagis_core::Message> {
                Some(message)
            }
        }
        assert!(!Signals.cancel(&pagis_core::RunId::from("absent".to_string())));
    }
}
