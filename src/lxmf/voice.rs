//! Codec2 voice messages, decoded to WAV files that play anywhere.
//!
//! LXMF's audio field (`FIELD_AUDIO`) is `[mode, bytes]`. For the Codec2
//! modes the bytes are the encoded frames one after another, with no header
//! (as MeshChat and Columba send and read them), at the mode's frame size.
//! The pure-Rust `codec2` crate decodes the 3200, 2400, 1600, 1400, 1300 and
//! 1200 modes, the ones clients record; 700C, 450 and 450PWB aren't in it,
//! so those recordings are kept as they came.

use codec2::{Codec2, Codec2Mode};
use lxmf_core::constants::{
    AM_CODEC2_1200, AM_CODEC2_1300, AM_CODEC2_1400, AM_CODEC2_1600, AM_CODEC2_2400, AM_CODEC2_3200,
};

/// Codec2's output: 8 kHz, mono, 16-bit.
const SAMPLE_RATE: u32 = 8000;
/// Longest recording decoded (ten minutes), so a message can't make rettui
/// write gigabytes.
const MAX_SECONDS: usize = 600;

/// The Codec2 mode an LXMF audio mode is, if it can be decoded.
fn codec2_mode(mode: u8) -> Option<Codec2Mode> {
    Some(match mode {
        AM_CODEC2_1200 => Codec2Mode::MODE_1200,
        AM_CODEC2_1300 => Codec2Mode::MODE_1300,
        AM_CODEC2_1400 => Codec2Mode::MODE_1400,
        AM_CODEC2_1600 => Codec2Mode::MODE_1600,
        AM_CODEC2_2400 => Codec2Mode::MODE_2400,
        AM_CODEC2_3200 => Codec2Mode::MODE_3200,
        _ => return None,
    })
}

/// A Codec2 recording as a WAV file, if its mode can be decoded and it has
/// a whole frame (a part frame at the end is left out).
pub fn codec2_wav(mode: u8, data: &[u8]) -> Option<Vec<u8>> {
    let mut codec = Codec2::new(codec2_mode(mode)?);
    let frame_bytes = codec.bits_per_frame().div_ceil(8);
    let frame_samples = codec.samples_per_frame();
    let max_frames = MAX_SECONDS * SAMPLE_RATE as usize / frame_samples;
    let frames = (data.len() / frame_bytes).min(max_frames);
    if frames == 0 {
        return None;
    }
    let mut samples = vec![0i16; frames * frame_samples];
    for (bits, speech) in data.chunks_exact(frame_bytes).zip(samples.chunks_exact_mut(frame_samples)) {
        codec.decode(speech, bits);
    }
    Some(wav(&samples))
}

/// Samples as a WAV file (PCM, 8 kHz mono, 16-bit little-endian).
fn wav(samples: &[i16]) -> Vec<u8> {
    let data_len = u32::try_from(samples.len() * 2).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes()); // bytes a second
    out.extend_from_slice(&2u16.to_le_bytes()); // bytes a sample
    out.extend_from_slice(&16u16.to_le_bytes()); // bits a sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

/// Whether bytes are a WAV file.
pub fn is_wav(data: &[u8]) -> bool {
    data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WAVE"
}

#[cfg(test)]
mod tests {
    use lxmf_core::constants::{AM_CODEC2_700C, AM_OPUS_OGG};

    use super::*;

    /// Four seconds of a synthetic vowel-like signal, encoded at 1200 bps by
    /// Codec2's reference encoder (`c2enc 1200`): 100 frames of 6 bytes.
    const BITS_1200: &[u8] = include_bytes!("testdata/codec2-1200.bin");

    /// The loudness (RMS) of each 40 ms frame the reference decoder
    /// (`c2dec 1200`) makes of them.
    const REFERENCE_RMS: [u32; 100] = [
        1162, 2741, 2863, 3031, 3620, 3985, 3921, 3911, 4075, 3881, 4572, 4673, 5101, 5217, 4327, 3721, 3674, 4722,
        4278, 1772, 400, 415, 304, 314, 264, 2091, 2303, 2138, 2091, 1683, 1722, 1668, 2370, 2275, 2130, 2239, 2655,
        2595, 2817, 2531, 2709, 2886, 2562, 2525, 747, 216, 213, 242, 255, 232, 1617, 2729, 2273, 2456, 3354, 3537,
        3359, 3277, 3578, 4082, 3541, 3731, 4408, 4466, 4212, 3828, 3709, 3912, 3102, 1566, 433, 454, 451, 376, 259,
        1995, 3211, 2793, 2586, 2330, 2433, 2259, 2408, 2600, 2731, 2587, 2366, 2716, 2777, 2468, 2295, 2073, 2302,
        2006, 633, 179, 219, 257, 200, 170,
    ];

    #[test]
    fn codec2_decodes_as_the_reference_decoder_does() {
        let wav = codec2_wav(AM_CODEC2_1200, BITS_1200).unwrap();
        assert!(is_wav(&wav));
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 8000);
        let samples: Vec<f64> = wav[44..].chunks(2).map(|b| f64::from(i16::from_le_bytes([b[0], b[1]]))).collect();
        assert_eq!(samples.len(), 32_000, "100 frames of 320 samples: four seconds");
        // Not the same samples (unvoiced sounds get random phases), but the
        // same sound: each frame as loud as the reference decoder's.
        let rms: Vec<f64> = samples.chunks(320).map(|f| (f.iter().map(|s| s * s).sum::<f64>() / 320.0).sqrt()).collect();
        let reference: Vec<f64> = REFERENCE_RMS.iter().map(|&r| f64::from(r)).collect();
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        let (ma, mb) = (mean(&rms), mean(&reference));
        let cov: f64 = rms.iter().zip(&reference).map(|(a, b)| (a - ma) * (b - mb)).sum();
        let var = |v: &[f64], m: f64| v.iter().map(|x| (x - m) * (x - m)).sum::<f64>();
        let correlation = cov / (var(&rms, ma) * var(&reference, mb)).sqrt();
        assert!(correlation > 0.99, "loudness follows the reference: {correlation}");
        for (ours, theirs) in rms.iter().zip(&reference).filter(|(_, r)| **r > 1000.0) {
            assert!((0.7..1.4).contains(&(ours / theirs)), "{ours} against {theirs}");
        }
    }

    #[test]
    fn what_isnt_decoded() {
        assert!(codec2_wav(AM_CODEC2_700C, &[0; 40]).is_none(), "700C isn't in the decoder");
        assert!(codec2_wav(AM_OPUS_OGG, b"OggS").is_none());
        assert!(codec2_wav(AM_CODEC2_1200, &[0; 5]).is_none(), "not a whole frame");
        // A part frame at the end is left out.
        let wav = codec2_wav(AM_CODEC2_3200, &[0; 8 * 3 + 5]).unwrap();
        assert_eq!(wav.len(), 44 + 3 * 160 * 2);
        assert!(codec2_wav(AM_CODEC2_2400, &[0; 6]).is_some());
    }
}
