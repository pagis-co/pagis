//! Calendar intake surrounds the supplied Workspace Mailbox store.
use async_trait::async_trait;

use crate::{
    FlagChange, MailReadContext, MessageId, OutgoingAttachment, OutgoingMessage, Page, PageRequest,
    SentMessage, TransportError, WorkspaceMailMessage, WorkspaceMailReader, WorkspaceMailStore,
    WorkspaceMailSummary, WorkspaceMailWriter,
};

pub(crate) struct CalendarMailStore {
    pub inner: Box<dyn WorkspaceMailStore>,
    pub context: MailReadContext,
}

#[async_trait]
impl WorkspaceMailReader for CalendarMailStore {
    async fn quarantined_message(
        &mut self,
        id: &MessageId,
    ) -> Result<WorkspaceMailMessage, TransportError> {
        self.inner.quarantined_message(id).await
    }
    async fn junk_folder(&mut self) -> Result<String, TransportError> {
        self.inner.junk_folder().await
    }
    async fn attachment(
        &mut self,
        id: &MessageId,
        index: usize,
    ) -> Result<OutgoingAttachment, TransportError> {
        self.inner.attachment(id, index).await
    }
    async fn folders(&mut self) -> Result<Vec<String>, TransportError> {
        self.inner.folders().await
    }
    async fn messages(
        &mut self,
        folder: &str,
        q: &str,
        page: &PageRequest,
    ) -> Result<Page<WorkspaceMailSummary>, TransportError> {
        self.inner.messages(folder, q, page).await
    }
    async fn message(&mut self, id: &MessageId) -> Result<WorkspaceMailMessage, TransportError> {
        let message = self.inner.message(id).await?;
        self.context.calendar_message(&message.message).await?;
        Ok(message)
    }
    async fn thread(&mut self, id: &str) -> Result<Vec<WorkspaceMailMessage>, TransportError> {
        let messages = self.inner.thread(id).await?;
        for message in &messages {
            self.context.calendar_message(&message.message).await?;
        }
        Ok(messages)
    }
    async fn set_flags(&mut self, id: &MessageId, flags: FlagChange) -> Result<(), TransportError> {
        self.inner.set_flags(id, flags).await
    }
    async fn close(&mut self) -> Result<(), TransportError> {
        self.inner.close().await
    }
}

#[async_trait]
impl WorkspaceMailWriter for CalendarMailStore {
    async fn mark_spam(&mut self, id: &MessageId, spam: bool) -> Result<(), TransportError> {
        self.inner.mark_spam(id, spam).await
    }
    async fn send(&mut self, message: &OutgoingMessage) -> Result<SentMessage, TransportError> {
        self.inner.send(message).await
    }
    async fn create_folder(&mut self, name: &str) -> Result<(), TransportError> {
        self.inner.create_folder(name).await
    }
    async fn rename_folder(&mut self, name: &str, new_name: &str) -> Result<(), TransportError> {
        self.inner.rename_folder(name, new_name).await
    }
    async fn delete_folder(&mut self, name: &str) -> Result<(), TransportError> {
        self.inner.delete_folder(name).await
    }
    async fn move_message(&mut self, id: &MessageId, folder: &str) -> Result<(), TransportError> {
        self.inner.move_message(id, folder).await
    }
    async fn set_label(
        &mut self,
        id: &MessageId,
        label: &str,
        apply: bool,
    ) -> Result<(), TransportError> {
        self.inner.set_label(id, label, apply).await
    }
}
