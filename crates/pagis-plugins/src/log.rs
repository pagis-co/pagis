//! The stderr log of a Plugin (ADR-0017).
//!
//! A stdio server's stderr goes to one file for each Workspace and
//! Plugin, and the desk reads the end of the file of its own Workspace.
//! A server can write Person data or a bound value to stderr, so no
//! Person reads the log of another Workspace (ADR-0023). Nothing of it
//! reaches the model (ADR-0005). The file has a fixed size, in the
//! style of Docker's
//! `json-file` log driver with `max-size`: each write keeps it at or
//! under [`MAX_LOG_BYTES`]. When the output does not fit, the log takes
//! what fits, drops the rest and ends with one [`LOG_FULL_MARKER`]
//! line. Stderr is diagnostic output, so a lost tail does not stop the
//! server.

use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use pagis_core::{PluginId, WorkspaceId};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeek, AsyncSeekExt, AsyncWriteExt};

/// The largest stderr log one Plugin keeps: 1 MiB. Each write keeps
/// the log at or under this size. When the budget is spent, the log
/// drops the rest of the output and ends with one
/// [`LOG_FULL_MARKER`] line. The next start of a server of the Plugin
/// empties a full log.
pub const MAX_LOG_BYTES: u64 = 1024 * 1024;

/// The line a full log ends with. The output after it is dropped.
pub const LOG_FULL_MARKER: &str =
    "[pagis] The log is full. Pagis drops the output after this line.\n";

/// The most output a log holds before its marker. The one byte more
/// is for the newline that starts the marker after a cut line.
const OUTPUT_BYTES: u64 = MAX_LOG_BYTES - LOG_FULL_MARKER.len() as u64 - 1;

/// The lock of each log file, by the Workspace and the Plugin it
/// belongs to.
type FileLocks = HashMap<(WorkspaceId, PluginId), Arc<tokio::sync::Mutex<()>>>;

/// The stderr logs of the Plugins: `<directory>/<workspace id>/<plugin
/// id>.log`, one file for each Workspace and Plugin.
///
/// Every server of one Plugin in one Workspace writes the same file,
/// and a restart can start a new server while the old one still
/// writes. The writes to one file therefore take one lock, and that
/// lock is what holds the file to its size when two servers write at
/// the same time.
pub struct PluginLogs {
    directory: PathBuf,
    locks: Mutex<FileLocks>,
}

impl PluginLogs {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            locks: Mutex::new(HashMap::new()),
        }
    }

    /// Where the log of one Plugin in one Workspace is.
    pub fn path(&self, workspace_id: &WorkspaceId, plugin_id: &PluginId) -> PathBuf {
        self.directory
            .join(workspace_id.as_str())
            .join(format!("{plugin_id}.log"))
    }

    /// The log of one Plugin in one Workspace, for a server of it to
    /// write.
    pub fn log(&self, workspace_id: &WorkspaceId, plugin_id: &PluginId) -> PluginLog {
        let lock = Arc::clone(
            self.locks
                .lock()
                .expect("the plugin log lock map")
                .entry((workspace_id.clone(), plugin_id.clone()))
                .or_default(),
        );
        PluginLog {
            path: self.path(workspace_id, plugin_id),
            lock,
        }
    }

    /// The end of the log of one Plugin in one Workspace, at most
    /// `limit` bytes. A log that is not there is empty.
    pub async fn tail(
        &self,
        workspace_id: &WorkspaceId,
        plugin_id: &PluginId,
        limit: usize,
    ) -> String {
        let Ok(file) = tokio::fs::File::open(self.path(workspace_id, plugin_id)).await else {
            return String::new();
        };
        tail_of(file, limit).await.unwrap_or_default()
    }

    /// Delete the logs of one Plugin in every Workspace. An uninstall
    /// removes the Plugin from every Workspace, also from a Workspace
    /// whose host this daemon has not made since it started.
    pub async fn remove(&self, plugin_id: &PluginId) {
        let file = format!("{plugin_id}.log");
        if let Ok(mut workspaces) = tokio::fs::read_dir(&self.directory).await {
            while let Ok(Some(workspace)) = workspaces.next_entry().await {
                let _ = tokio::fs::remove_file(workspace.path().join(&file)).await;
            }
        }
        self.locks
            .lock()
            .expect("the plugin log lock map")
            .retain(|(_, id), _| id != plugin_id);
    }
}

/// The log of one Plugin in one Workspace, as the servers of the
/// Plugin in that Workspace write it.
#[derive(Clone)]
pub struct PluginLog {
    path: PathBuf,
    lock: Arc<tokio::sync::Mutex<()>>,
}

impl PluginLog {
    /// Copy one server's stderr into the log, in a task of its own,
    /// until the stream ends.
    ///
    /// The start empties a full log. Each write then keeps the log at
    /// or under [`MAX_LOG_BYTES`]: output that does not fit is dropped,
    /// and the log ends with one [`LOG_FULL_MARKER`] line. The task
    /// reads the stream to its end, also when the log is full or cannot
    /// be written. The exec stream waits for this reader, so a reader
    /// that stops also stops the server's stdout.
    pub fn record_stderr(&self, server: &str, mut stderr: tokio::sync::mpsc::Receiver<Vec<u8>>) {
        let log = self.clone();
        let server = server.to_string();
        tokio::spawn(async move {
            let mut file = match log.open().await {
                Ok(file) => Some(file),
                Err(error) => {
                    tracing::warn!(%error, log = %log.path.display(), "the plugin log does not open");
                    None
                }
            };
            while let Some(chunk) = stderr.recv().await {
                let Some(open) = file.as_mut() else {
                    continue;
                };
                let text: String = String::from_utf8_lossy(&chunk)
                    .lines()
                    .map(|line| format!("[{server}] {line}\n"))
                    .collect();
                if let Err(error) = log.append(open, &text).await {
                    tracing::warn!(%error, log = %log.path.display(), "the plugin log takes no more output");
                    file = None;
                }
            }
        });
    }

    /// Open the log for one server start. A full log starts again from
    /// empty.
    async fn open(&self) -> std::io::Result<tokio::fs::File> {
        let _held = self.lock.lock().await;
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await?;
        if file.metadata().await?.len() > OUTPUT_BYTES {
            file.set_len(0).await?;
        }
        Ok(file)
    }

    /// Append one piece of output: all of it when it fits, or else the
    /// part that fits and the marker. A full log takes nothing.
    ///
    /// Every piece of output ends with a newline, so a log that has
    /// taken only whole pieces ends with one too. A cut piece fills the
    /// log to within three bytes of [`OUTPUT_BYTES`], and the marker
    /// after it takes the log past that size. A log over that size is
    /// therefore full.
    async fn append(&self, file: &mut tokio::fs::File, text: &str) -> std::io::Result<()> {
        let _held = self.lock.lock().await;
        let held = file.metadata().await?.len();
        if held > OUTPUT_BYTES {
            return Ok(());
        }
        let room = usize::try_from(OUTPUT_BYTES - held).unwrap_or(usize::MAX);
        if text.len() <= room {
            file.write_all(text.as_bytes()).await?;
        } else {
            let fits = &text[..text.floor_char_boundary(room)];
            let mut last = String::with_capacity(fits.len() + 1 + LOG_FULL_MARKER.len());
            last.push_str(fits);
            if !fits.is_empty() && !fits.ends_with('\n') {
                last.push('\n');
            }
            last.push_str(LOG_FULL_MARKER);
            file.write_all(last.as_bytes()).await?;
        }
        // A tokio file writes in the background. The flush waits for
        // the write, so the size the next append reads is true.
        file.flush().await
    }
}

/// The end of one log, at most `limit` bytes. It seeks to that end and
/// reads nothing before it, so a large log costs no more than `limit`.
/// The end starts at a whole character.
pub async fn tail_of<R>(mut reader: R, limit: usize) -> std::io::Result<String>
where
    R: AsyncRead + AsyncSeek + Unpin,
{
    let end = reader.seek(SeekFrom::End(0)).await?;
    let start = end.saturating_sub(u64::try_from(limit).unwrap_or(u64::MAX));
    reader.seek(SeekFrom::Start(start)).await?;
    let mut bytes = Vec::with_capacity(usize::try_from(end - start).unwrap_or(limit));
    (&mut reader)
        .take(end - start)
        .read_to_end(&mut bytes)
        .await?;
    // The first bytes can be the rest of a character that starts
    // before the end: a UTF-8 continuation byte is `10xxxxxx`.
    let partial = if start == 0 {
        0
    } else {
        bytes
            .iter()
            .take(3)
            .take_while(|byte| **byte & 0xC0 == 0x80)
            .count()
    };
    Ok(String::from_utf8_lossy(&bytes[partial..]).into_owned())
}
