//! Recording (ADR-0020). Pagis holds the RTP, so the recording
//! is ours: no provider records the call.
//!
//! While the call runs, each direction is appended raw to its own file,
//! exactly as the G.711 bytes arrived. Nothing is muxed, buffered or
//! rewritten, so a daemon that is killed mid-call leaves two files that
//! the same [`mux`] step reads. The codec is in the file name, because
//! the mux step runs after a restart and has no other memory of it.
//!
//! At settle the two legs mux to one 8 kHz stereo WAV: the Remote Party
//! on the left channel and the Agent on the right. The shorter leg is
//! padded with silence, so the two channels stay the same length.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use pagis_core::CallId;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::audio::{Codec, SAMPLE_RATE};
use crate::hub::{Direction, Recorded, RecordingSink};

/// What the muxed recording is.
pub const RECORDING_MIME: &str = "audio/wav";
/// What the muxed recording is called in the Artifact row.
pub const RECORDING_FILENAME: &str = "recording.wav";

/// The Remote Party's leg, which becomes the left channel.
const REMOTE_LEG: &str = "remote";
/// The Agent's leg, which becomes the right channel.
const AGENT_LEG: &str = "agent";

/// Where one leg of one call is appended. The codec is the extension,
/// so the mux step reads it back with no other state.
fn leg_path(dir: &Path, call_id: &CallId, leg: &str, codec: Codec) -> PathBuf {
    dir.join(format!(
        "{}.{leg}.{}",
        call_id.as_str(),
        codec.name().to_lowercase()
    ))
}

/// The leg file of one call, whatever codec it was recorded in.
fn find_leg(dir: &Path, call_id: &CallId, leg: &str) -> Option<(PathBuf, Codec)> {
    [Codec::Pcmu, Codec::Pcma]
        .into_iter()
        .map(|codec| (leg_path(dir, call_id, leg, codec), codec))
        .find(|(path, _)| path.is_file())
}

/// The recorder of one call: two append-only files, one per direction.
pub struct CallRecorder {
    remote: Mutex<tokio::fs::File>,
    agent: Mutex<tokio::fs::File>,
}

impl CallRecorder {
    /// Open both legs of one call. An existing recording of the same
    /// call is replaced, because a call id names one call.
    pub async fn create(
        dir: &Path,
        call_id: &CallId,
        codec: Codec,
    ) -> Result<Self, std::io::Error> {
        tokio::fs::create_dir_all(dir).await?;
        Ok(Self {
            remote: Mutex::new(create(&leg_path(dir, call_id, REMOTE_LEG, codec)).await?),
            agent: Mutex::new(create(&leg_path(dir, call_id, AGENT_LEG, codec)).await?),
        })
    }
}

async fn create(path: &Path) -> Result<tokio::fs::File, std::io::Error> {
    tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .await
}

#[async_trait]
impl RecordingSink for CallRecorder {
    async fn write(&self, recorded: Recorded) {
        let leg = match recorded.direction {
            Direction::Uplink => &self.remote,
            Direction::Downlink => &self.agent,
        };
        // A disk that refuses the frame loses that frame and not the
        // call: the audio matters more than the recording.
        if let Err(error) = leg.lock().await.write_all(recorded.frame.payload()).await {
            tracing::warn!(%error, "a recorded frame did not reach the disk");
        }
    }
}

/// Mux the two legs of one call into one 8 kHz stereo WAV: the Remote
/// Party left, the Agent right. `None` when the call recorded nothing.
///
/// The step reads only the files, so it settles a call the daemon was
/// killed in the middle of exactly as it settles a call that ended.
pub async fn mux(dir: &Path, call_id: &CallId) -> Result<Option<Vec<u8>>, std::io::Error> {
    let remote = read_leg(dir, call_id, REMOTE_LEG).await?;
    let agent = read_leg(dir, call_id, AGENT_LEG).await?;
    if remote.is_empty() && agent.is_empty() {
        return Ok(None);
    }
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut wav = Cursor::new(Vec::new());
    let mut writer =
        hound::WavWriter::new(&mut wav, spec).map_err(|error| wav_error("write", error))?;
    for index in 0..remote.len().max(agent.len()) {
        // A leg that stopped early is silence from there on, so both
        // channels stay the same length.
        writer
            .write_sample(remote.get(index).copied().unwrap_or(0))
            .map_err(|error| wav_error("write", error))?;
        writer
            .write_sample(agent.get(index).copied().unwrap_or(0))
            .map_err(|error| wav_error("write", error))?;
    }
    writer
        .finalize()
        .map_err(|error| wav_error("finalize", error))?;
    Ok(Some(wav.into_inner()))
}

/// Delete the raw legs of one call. The WAV is the recording from here
/// on, so the G.711 files are not kept twice.
pub async fn discard(dir: &Path, call_id: &CallId) {
    for leg in [REMOTE_LEG, AGENT_LEG] {
        if let Some((path, _)) = find_leg(dir, call_id, leg)
            && let Err(error) = tokio::fs::remove_file(&path).await
        {
            tracing::warn!(%error, path = %path.display(), "a raw leg was not removed");
        }
    }
}

/// One leg as 16-bit samples. A leg with no file is no audio.
async fn read_leg(dir: &Path, call_id: &CallId, leg: &str) -> Result<Vec<i16>, std::io::Error> {
    let Some((path, codec)) = find_leg(dir, call_id, leg) else {
        return Ok(Vec::new());
    };
    let bytes = tokio::fs::read(&path).await?;
    Ok(bytes.iter().map(|byte| decode(codec, *byte)).collect())
}

fn decode(codec: Codec, byte: u8) -> i16 {
    match codec {
        Codec::Pcmu => audio_codec_algorithms::decode_ulaw(byte),
        Codec::Pcma => audio_codec_algorithms::decode_alaw(byte),
    }
}

fn wav_error(step: &str, error: hound::Error) -> std::io::Error {
    std::io::Error::other(format!("the recording did not {step}: {error}"))
}
