//! A WAV container for one PCM16 clip, so a buffered transcription can
//! post it to a provider that reads files and not raw samples.

use bytes::{BufMut, Bytes, BytesMut};

/// Wrap PCM16 little-endian mono samples in a 44-byte RIFF/WAVE header.
pub fn pcm16_wav(pcm16: &[u8], sample_rate: u32) -> Bytes {
    const CHANNELS: u16 = 1;
    const BITS_PER_SAMPLE: u16 = 16;
    let block_align = CHANNELS * BITS_PER_SAMPLE / 8;
    let byte_rate = sample_rate * u32::from(block_align);
    let data_len = u32::try_from(pcm16.len()).expect("a clip fits in a WAV data chunk");

    let mut wav = BytesMut::with_capacity(44 + pcm16.len());
    wav.put_slice(b"RIFF");
    wav.put_u32_le(36 + data_len);
    wav.put_slice(b"WAVE");
    wav.put_slice(b"fmt ");
    wav.put_u32_le(16);
    wav.put_u16_le(1); // PCM
    wav.put_u16_le(CHANNELS);
    wav.put_u32_le(sample_rate);
    wav.put_u32_le(byte_rate);
    wav.put_u16_le(block_align);
    wav.put_u16_le(BITS_PER_SAMPLE);
    wav.put_slice(b"data");
    wav.put_u32_le(data_len);
    wav.put_slice(pcm16);
    wav.freeze()
}

#[cfg(test)]
mod tests {
    use super::pcm16_wav;

    #[test]
    fn the_header_describes_mono_pcm16_at_the_given_rate() {
        let wav = pcm16_wav(&[1, 0, 2, 0], 24_000);
        assert_eq!(wav.len(), 48);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 40);
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(u16::from_le_bytes(wav[20..22].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 24_000);
        assert_eq!(u32::from_le_bytes(wav[28..32].try_into().unwrap()), 48_000);
        assert_eq!(u16::from_le_bytes(wav[32..34].try_into().unwrap()), 2);
        assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()), 16);
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 4);
        assert_eq!(&wav[44..], &[1, 0, 2, 0]);
    }
}
