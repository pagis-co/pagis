//! The desk reads the end of a Plugin's stderr log (ADR-0017). It
//! reads that end from the end of the file, and never the whole file.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};

use pagis_plugins::tail_of;
use tokio::io::{AsyncRead, AsyncSeek, ReadBuf};

/// A reader that counts the bytes that leave it.
struct Counting<R> {
    inner: R,
    read: Arc<AtomicUsize>,
}

impl<R: AsyncRead + Unpin> AsyncRead for Counting<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let polled = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = polled {
            self.read
                .fetch_add(buf.filled().len() - before, Ordering::SeqCst);
        }
        polled
    }
}

impl<R: AsyncSeek + Unpin> AsyncSeek for Counting<R> {
    fn start_seek(mut self: Pin<&mut Self>, position: std::io::SeekFrom) -> std::io::Result<()> {
        Pin::new(&mut self.inner).start_seek(position)
    }

    fn poll_complete(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<u64>> {
        Pin::new(&mut self.inner).poll_complete(cx)
    }
}

/// A log file, a reader of it that counts, and the count.
type Counted = (
    tempfile::TempDir,
    Counting<tokio::fs::File>,
    Arc<AtomicUsize>,
);

/// One log file with the text, and a reader of it that counts.
async fn counted(text: &str) -> Counted {
    let directory = tempfile::tempdir().expect("a scratch directory");
    let path = directory.path().join("plugin.log");
    std::fs::write(&path, text).expect("the log");
    let file = tokio::fs::File::open(&path).await.expect("the log opens");
    let read = Arc::new(AtomicUsize::new(0));
    let reader = Counting {
        inner: file,
        read: Arc::clone(&read),
    };
    (directory, reader, read)
}

#[tokio::test]
async fn the_desk_reads_the_end_of_a_log_larger_than_its_limit_and_no_more() {
    let text: String = (0..4000)
        .map(|line| format!("[weather] line {line}\n"))
        .collect();
    let limit = 4096;
    assert!(text.len() > 10 * limit);
    let (_directory, reader, read) = counted(&text).await;

    let tail = tail_of(reader, limit).await.expect("the log reads");

    assert_eq!(tail, text[text.len() - limit..]);
    let taken = read.load(Ordering::SeqCst);
    assert!(
        taken <= limit,
        "the read took {taken} bytes of a {} byte log for a limit of {limit}",
        text.len()
    );
}

#[tokio::test]
async fn the_end_of_a_log_starts_at_a_whole_character() {
    // Each "é" is two bytes, so a limit of an odd number of bytes
    // starts inside one character.
    let text = "é".repeat(100);
    let (_directory, reader, _read) = counted(&text).await;

    let tail = tail_of(reader, 9).await.expect("the log reads");

    assert_eq!(tail, "é".repeat(4));
}

#[tokio::test]
async fn a_log_under_its_limit_reads_whole() {
    let text = "[weather] one line\n";
    let (_directory, reader, _read) = counted(text).await;

    let tail = tail_of(reader, 4096).await.expect("the log reads");

    assert_eq!(tail, text);
}
