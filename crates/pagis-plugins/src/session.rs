//! One session interface for every Plugin server.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use rmcp::model::{CallToolRequestParams, CallToolResult, Tool};

/// A call that has no supported answer.
#[derive(Debug)]
pub enum SessionError {
    Lost(String),
    /// True for the first call that counts this session's output overrun.
    Overrun {
        first: bool,
    },
    Unsupported,
}

/// One connected Plugin server. Each call has a deadline.
#[async_trait]
pub trait Session: Send + Sync + std::fmt::Debug {
    async fn list_tools(&self) -> Result<Vec<Tool>, String>;
    async fn call(
        &self,
        params: CallToolRequestParams,
        deadline: Duration,
    ) -> Result<CallToolResult, SessionError>;
    fn is_closed(&self) -> bool;
    async fn close(&self);
}

/// One session's output overrun. Each session counts at most once.
#[derive(Debug, Default)]
pub(crate) struct Overrun {
    pub(crate) happened: AtomicBool,
    counted: AtomicBool,
}

impl Overrun {
    pub(crate) fn happened(&self) -> bool {
        self.happened.load(Ordering::SeqCst)
    }

    pub(crate) fn error(&self, message: String) -> SessionError {
        if self.happened() {
            SessionError::Overrun {
                first: !self.counted.swap(true, Ordering::SeqCst),
            }
        } else {
            SessionError::Lost(message)
        }
    }
}
