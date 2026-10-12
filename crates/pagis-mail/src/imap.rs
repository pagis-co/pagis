//! The byte driver for the sans-I/O IMAP client.

use std::time::Duration;

use imap_codec::{CommandCodec, decode::Decoder};
use imap_next::imap_types::IntoStatic;
use imap_next::{
    Interrupt, Io, State,
    client::{Client, Event, Options},
    imap_types::{
        command::{Command, CommandBody},
        core::Tag,
        response::{GreetingKind, Response, Status, StatusKind},
    },
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::transport::{IdleOutcome, TransportError, TransportErrorCode};

pub(crate) const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);

/// The client owns the grammar and protocol flow. The driver owns I/O.
pub(crate) struct Driver<S> {
    pub(crate) stream: S,
    client: Client,
    serial: u64,
    closed: bool,
    output: Vec<u8>,
    written: usize,
    idle_tag: Option<Tag<'static>>,
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send> Driver<S> {
    pub(crate) fn new(stream: S) -> Self {
        Self {
            stream,
            client: Client::new(Options::default()),
            serial: 0,
            closed: false,
            output: Vec::new(),
            written: 0,
            idle_tag: None,
        }
    }

    pub(crate) async fn greeting(&mut self) -> Result<(), TransportError> {
        timeout(async {
            match self.next().await? {
                Event::GreetingReceived { greeting } if greeting.kind == GreetingKind::Ok => Ok(()),
                _ => Err(TransportError(TransportErrorCode::Rejected)),
            }
        })
        .await
    }

    fn tag(&mut self) -> Tag<'static> {
        self.serial += 1;
        Tag::try_from(format!("A{}", self.serial)).expect("a generated IMAP tag is valid")
    }

    pub(crate) async fn command(
        &mut self,
        body: CommandBody<'static>,
    ) -> Result<Vec<Response<'static>>, TransportError> {
        let tag = self.tag();
        let refused = if matches!(body, CommandBody::Login { .. }) {
            TransportErrorCode::Unauthorized
        } else {
            TransportErrorCode::Rejected
        };
        self.client.enqueue_command(Command {
            tag: tag.clone(),
            body,
        });
        timeout(self.responses(tag, false, refused)).await
    }

    /// Decode generated commands with the library's grammar. User strings
    /// in these commands are quoted before they reach this function.
    pub(crate) async fn command_text(
        &mut self,
        body: &str,
    ) -> Result<Vec<Response<'static>>, TransportError> {
        let text = format!("{} {body}\r\n", self.tag().inner());
        let (rest, command) = CommandCodec::default()
            .decode(text.as_bytes())
            .map_err(|_| TransportError(TransportErrorCode::Rejected))?;
        if !rest.is_empty() {
            return Err(TransportError(TransportErrorCode::Rejected));
        }
        let command = command.into_static();
        let tag = command.tag.clone();
        self.client.enqueue_command(command);
        timeout(self.responses(tag, false, TransportErrorCode::Rejected)).await
    }

    pub(crate) async fn logout(&mut self) -> Result<(), TransportError> {
        let result = if self.closed {
            Ok(())
        } else {
            async {
                // Complete IDLE before LOGOUT. A server can send its IDLE
                // result before it reads another command.
                self.end_idle().await?;
                let tag = self.tag();
                self.client.enqueue_command(Command {
                    tag: tag.clone(),
                    body: CommandBody::Logout,
                });
                timeout(self.responses(tag, true, TransportErrorCode::Rejected))
                    .await
                    .map(|_| ())
            }
            .await
        };
        self.closed = true;
        // Close even when the server refuses LOGOUT or does not answer it.
        let _ = timeout(async { self.stream.shutdown().await.map_err(unreachable) }).await;
        result
    }

    async fn responses(
        &mut self,
        tag: Tag<'static>,
        logout: bool,
        refused: TransportErrorCode,
    ) -> Result<Vec<Response<'static>>, TransportError> {
        let mut responses = Vec::new();
        loop {
            match self.next().await? {
                // A cancelled IDLE can still wait for its continuation.
                Event::IdleAccepted { .. } => {
                    self.client.set_idle_done();
                }
                Event::DataReceived { data } => responses.push(Response::Data(data)),
                Event::StatusReceived {
                    status: Status::Tagged(status),
                } if status.tag == tag => {
                    check(status.body.kind, refused)?;
                    responses.push(Response::Status(Status::Tagged(status)));
                    return Ok(responses);
                }
                Event::StatusReceived {
                    status: Status::Bye(_),
                } if !logout => {
                    return Err(TransportError(TransportErrorCode::Unreachable));
                }
                Event::StatusReceived { status } => responses.push(Response::Status(status)),
                Event::CommandRejected { status, .. } | Event::IdleRejected { status, .. } => {
                    return Err(status_error(&status, refused));
                }
                _ => {}
            }
        }
    }

    pub(crate) async fn idle(&mut self, turn: Duration) -> Result<IdleOutcome, TransportError> {
        let tag = self.tag();
        self.client.enqueue_command(Command {
            tag: tag.clone(),
            body: CommandBody::Idle,
        });
        self.idle_tag = Some(tag);
        timeout(async {
            loop {
                match self.next().await? {
                    Event::IdleAccepted { .. } => return Ok(()),
                    Event::IdleRejected { status, .. } => {
                        return Err(status_error(&status, TransportErrorCode::Rejected));
                    }
                    Event::StatusReceived {
                        status: Status::Bye(_),
                    } => return Err(TransportError(TransportErrorCode::Unreachable)),
                    _ => {}
                }
            }
        })
        .await?;
        let outcome = match tokio::time::timeout(turn, async {
            loop {
                match self.next().await? {
                    Event::DataReceived { .. } => return Ok(IdleOutcome::Changed),
                    Event::StatusReceived {
                        status: Status::Bye(_),
                    } => return Err(TransportError(TransportErrorCode::Unreachable)),
                    _ => {}
                }
            }
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Ok(IdleOutcome::Deadline),
        };
        let ended = self.end_idle().await;
        outcome.and_then(|outcome| ended.map(|_| outcome))
    }

    async fn end_idle(&mut self) -> Result<(), TransportError> {
        let Some(tag) = self.idle_tag.clone() else {
            return Ok(());
        };
        self.client.set_idle_done();
        let result = timeout(self.responses(tag, false, TransportErrorCode::Rejected)).await;
        // Keep the tag until the exchange ends, so a cancelled DONE wait
        // can continue at logout.
        self.idle_tag = None;
        result.map(|_| ())
    }

    /// Progress the client until it emits an event. The state machine
    /// retains all partial input. No protocol parsing takes place here.
    async fn next(&mut self) -> Result<Event, TransportError> {
        if self.closed {
            return Err(TransportError(TransportErrorCode::Unreachable));
        }
        loop {
            // Keep partial output in the driver so cancellation loses no bytes.
            while self.written < self.output.len() {
                let count = self
                    .stream
                    .write(&self.output[self.written..])
                    .await
                    .map_err(unreachable)?;
                if count == 0 {
                    return Err(TransportError(TransportErrorCode::Unreachable));
                }
                self.written += count;
            }
            if !self.output.is_empty() {
                self.stream.flush().await.map_err(unreachable)?;
                self.output.clear();
                self.written = 0;
            }
            match self.client.next() {
                Ok(event) => return Ok(event),
                Err(Interrupt::Io(Io::Output(bytes))) => {
                    self.output = bytes;
                }
                Err(Interrupt::Io(Io::NeedMoreInput)) => {
                    let mut bytes = [0; 8192];
                    let count = self.stream.read(&mut bytes).await.map_err(unreachable)?;
                    if count == 0 {
                        return Err(TransportError(TransportErrorCode::Unreachable));
                    }
                    self.client.enqueue_input(&bytes[..count]);
                }
                Err(Interrupt::Error(_)) => {
                    return Err(TransportError(TransportErrorCode::Unreadable));
                }
            }
        }
    }
}

pub(crate) async fn timeout<T>(
    work: impl std::future::Future<Output = Result<T, TransportError>>,
) -> Result<T, TransportError> {
    tokio::time::timeout(COMMAND_TIMEOUT, work)
        .await
        .map_err(|_| TransportError(TransportErrorCode::Unreachable))?
}

fn unreachable(_: std::io::Error) -> TransportError {
    TransportError(TransportErrorCode::Unreachable)
}

fn check(kind: StatusKind, refused: TransportErrorCode) -> Result<(), TransportError> {
    match kind {
        StatusKind::Ok => Ok(()),
        StatusKind::No => Err(TransportError(refused)),
        StatusKind::Bad => Err(TransportError(TransportErrorCode::Rejected)),
    }
}

fn status_error(status: &Status<'_>, refused: TransportErrorCode) -> TransportError {
    match status {
        Status::Bye(_) => TransportError(TransportErrorCode::Unreachable),
        Status::Tagged(status) if status.body.kind == StatusKind::No => TransportError(refused),
        _ => TransportError(TransportErrorCode::Rejected),
    }
}
