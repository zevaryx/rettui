//! Codec2 voice messages, decoded to WAV files that play anywhere.
//!
//! LXMF's audio field (`FIELD_AUDIO`) is `[mode, bytes]`. For the Codec2
//! modes the bytes are the encoded frames one after another, with no header
//! (as MeshChat and Columba send and read them), at the mode's frame size.
//! The `codec2` crate (vendored in `vendor/codec2`, the pure-Rust port with
//! 700C, 450 and 450PWB added from Codec2's C sources) decodes every mode,
//! and encodes all but 450 and 450PWB, which Codec2 itself only decodes
//! well.

use codec2::{Codec2, Codec2Mode};
use lxmf_core::constants::{
    AM_CODEC2_450, AM_CODEC2_450PWB, AM_CODEC2_700C, AM_CODEC2_1200, AM_CODEC2_1300, AM_CODEC2_1400, AM_CODEC2_1600, AM_CODEC2_2400,
    AM_CODEC2_3200,
};

/// Codec2's sound: 8 kHz, mono, 16-bit (450PWB decodes to 16 kHz).
const SAMPLE_RATE: u32 = 8000;
/// Longest recording decoded (ten minutes), so a message can't make rettui
/// write gigabytes.
const MAX_SECONDS: usize = 600;

/// The Codec2 mode an LXMF audio mode is, if it's Codec2.
fn codec2_mode(mode: u8) -> Option<Codec2Mode> {
    Some(match mode {
        AM_CODEC2_450PWB => Codec2Mode::MODE_450PWB,
        AM_CODEC2_450 => Codec2Mode::MODE_450,
        AM_CODEC2_700C => Codec2Mode::MODE_700C,
        AM_CODEC2_1200 => Codec2Mode::MODE_1200,
        AM_CODEC2_1300 => Codec2Mode::MODE_1300,
        AM_CODEC2_1400 => Codec2Mode::MODE_1400,
        AM_CODEC2_1600 => Codec2Mode::MODE_1600,
        AM_CODEC2_2400 => Codec2Mode::MODE_2400,
        AM_CODEC2_3200 => Codec2Mode::MODE_3200,
        _ => return None,
    })
}

/// A Codec2 recording as a WAV file, if it's Codec2 and has a whole frame
/// (a part frame at the end is left out).
pub fn codec2_wav(mode: u8, data: &[u8]) -> Option<Vec<u8>> {
    let mut codec = Codec2::new(codec2_mode(mode)?);
    let frame_bytes = codec.bits_per_frame().div_ceil(8);
    let frame_samples = codec.samples_per_frame();
    let rate = if mode == AM_CODEC2_450PWB { 2 * SAMPLE_RATE } else { SAMPLE_RATE };
    let max_frames = MAX_SECONDS * rate as usize / frame_samples;
    let frames = (data.len() / frame_bytes).min(max_frames);
    if frames == 0 {
        return None;
    }
    let mut samples = vec![0i16; frames * frame_samples];
    for (bits, speech) in data.chunks_exact(frame_bytes).zip(samples.chunks_exact_mut(frame_samples)) {
        codec.decode(speech, bits);
    }
    Some(wav(&samples, rate))
}

/// Samples as a WAV file (PCM, mono, 16-bit little-endian, at `rate`).
fn wav(samples: &[i16], rate: u32) -> Vec<u8> {
    let data_len = u32::try_from(samples.len() * 2).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes()); // bytes a second
    out.extend_from_slice(&2u16.to_le_bytes()); // bytes a sample
    out.extend_from_slice(&16u16.to_le_bytes()); // bits a sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

/// The modes voice messages recorded here can go in (the `voice_mode`
/// setting), clearest first: 3200 bit/s is 400 bytes a second (a minute is
/// 24 KB), 700C is 88 (a minute is 5 KB, for LoRa). Sideband, MeshChat and
/// Columba play them all.
pub const SEND_MODES: &[&str] = &["3200", "2400", "1600", "1400", "1300", "1200", "700C"];

/// The LXMF audio mode of one of [`SEND_MODES`] (3200 for anything else).
pub fn send_mode(name: &str) -> u8 {
    match name {
        "2400" => AM_CODEC2_2400,
        "1600" => AM_CODEC2_1600,
        "1400" => AM_CODEC2_1400,
        "1300" => AM_CODEC2_1300,
        "1200" => AM_CODEC2_1200,
        "700C" => AM_CODEC2_700C,
        _ => AM_CODEC2_3200,
    }
}

/// The LXMF audio mode a codec is, as [`super::fields::Audio::codec`]
/// names it ("Codec2 700C"), if it's Codec2.
pub fn mode_named(codec: &str) -> Option<u8> {
    (0x01..=0x09).find(|&mode| super::fields::Audio::new(mode, Vec::new()).codec() == codec)
}
/// Longest recording sent (five minutes).
pub const MAX_SEND_SECONDS: usize = 300;

/// A WAV file's samples, if it's what a recording is sent from: PCM,
/// 16-bit, mono, 8 kHz.
pub fn wav_samples(data: &[u8]) -> Result<Vec<i16>, String> {
    if !is_wav(data) {
        return Err("Not a WAV file".into());
    }
    let (mut format, mut samples) = (None, None);
    let mut at = 12;
    while at + 8 <= data.len() {
        let id = &data[at..at + 4];
        let len = u32::from_le_bytes(data[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = &data[at + 8..(at + 8 + len).min(data.len())];
        match id {
            b"fmt " if body.len() >= 16 => {
                let word = |i: usize| u16::from_le_bytes([body[i], body[i + 1]]);
                let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                format = Some((word(0), word(2), rate, word(14)));
            }
            b"data" => samples = Some(body),
            _ => {}
        }
        // Chunks are padded to an even length.
        at += 8 + len + (len & 1);
    }
    match (format, samples) {
        (Some((1, 1, SAMPLE_RATE, 16)), Some(body)) => Ok(body.as_chunks::<2>().0.iter().map(|&b| i16::from_le_bytes(b)).collect()),
        (Some(_), Some(_)) => Err("A recording is sent from 16-bit mono 8 kHz PCM".into()),
        _ => Err("Not a WAV file with sound in it".into()),
    }
}

/// Samples (8 kHz mono) as Codec2 frames in `mode`: whole frames, the last
/// one filled out with silence; at most [`MAX_SEND_SECONDS`]. Not for 450
/// and 450PWB, which Codec2 doesn't encode well.
pub fn codec2_encode(mode: u8, samples: &[i16]) -> Option<Vec<u8>> {
    if matches!(mode, AM_CODEC2_450 | AM_CODEC2_450PWB) {
        return None;
    }
    let mut codec = Codec2::new(codec2_mode(mode)?);
    let frame_bytes = codec.bits_per_frame().div_ceil(8);
    let frame_samples = codec.samples_per_frame();
    let samples = &samples[..samples.len().min(MAX_SEND_SECONDS * SAMPLE_RATE as usize)];
    if samples.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(samples.len().div_ceil(frame_samples) * frame_bytes);
    let mut frame = vec![0i16; frame_samples];
    let mut bits = vec![0u8; frame_bytes];
    for chunk in samples.chunks(frame_samples) {
        frame[..chunk.len()].copy_from_slice(chunk);
        frame[chunk.len()..].fill(0);
        codec.encode(&mut bits, &frame);
        out.extend_from_slice(&bits);
    }
    Some(out)
}

/// Whether bytes are a WAV file.
pub fn is_wav(data: &[u8]) -> bool {
    data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WAVE"
}

#[cfg(test)]
mod tests {
    use lxmf_core::constants::AM_OPUS_OGG;

    use super::*;

    /// Four seconds of a synthetic vowel-like signal, encoded at 1200 bps by
    /// Codec2's reference encoder (`c2enc 1200`): 100 frames of 6 bytes.
    const BITS_1200: &[u8] = include_bytes!("testdata/codec2-1200.bin");

    /// The loudness (RMS) of each 40 ms frame the reference decoder
    /// (`c2dec 1200`) makes of them.
    const REFERENCE_RMS: [u32; 100] = [
        1162, 2741, 2863, 3031, 3620, 3985, 3921, 3911, 4075, 3881, 4572, 4673, 5101, 5217, 4327, 3721, 3674, 4722, 4278, 1772, 400, 415,
        304, 314, 264, 2091, 2303, 2138, 2091, 1683, 1722, 1668, 2370, 2275, 2130, 2239, 2655, 2595, 2817, 2531, 2709, 2886, 2562, 2525,
        747, 216, 213, 242, 255, 232, 1617, 2729, 2273, 2456, 3354, 3537, 3359, 3277, 3578, 4082, 3541, 3731, 4408, 4466, 4212, 3828, 3709,
        3912, 3102, 1566, 433, 454, 451, 376, 259, 1995, 3211, 2793, 2586, 2330, 2433, 2259, 2408, 2600, 2731, 2587, 2366, 2716, 2777,
        2468, 2295, 2073, 2302, 2006, 633, 179, 219, 257, 200, 170,
    ];

    /// Codec2's own test recording (hts1a: three seconds of a man saying
    /// "a while for some reason"), encoded by its reference encoder at 700C
    /// (75 frames of 4 bytes) and at 450 (75 of 3).
    const BITS_700C: &[u8] = include_bytes!("testdata/codec2-700c.bin");
    const BITS_450: &[u8] = include_bytes!("testdata/codec2-450.bin");

    /// The loudness (RMS) of each 40 ms the reference decoder makes of
    /// them: `c2dec 700C`, `c2dec 450` and `c2dec 450PWB`.
    const REFERENCE_RMS_700C: [u32; 75] = [
        14, 28, 35, 51, 95, 68, 360, 2623, 2697, 1771, 1726, 4126, 4265, 4431, 1918, 484, 589, 1360, 1343, 2939, 2030, 124, 88, 389, 1141,
        3579, 3060, 178, 67, 59, 166, 33, 394, 1923, 1301, 216, 502, 2251, 1273, 253, 180, 118, 270, 708, 2672, 4628, 7509, 3855, 542,
        1180, 64, 77, 35, 88, 416, 1549, 1282, 914, 679, 123, 347, 536, 308, 47, 40, 41, 46, 51, 40, 41, 40, 74, 58, 28, 33,
    ];
    const REFERENCE_RMS_450: [u32; 75] = [
        10, 14, 12, 20, 29, 19, 204, 2223, 1401, 911, 1749, 1732, 2107, 2770, 2246, 720, 1012, 1838, 878, 2020, 1305, 47, 483, 693, 766,
        2263, 2401, 131, 59, 110, 111, 15, 345, 1864, 729, 131, 334, 2540, 1552, 227, 124, 105, 220, 717, 2106, 2894, 4000, 2386, 451, 337,
        57, 30, 19, 345, 912, 2270, 1265, 1234, 614, 98, 222, 490, 374, 29, 14, 14, 14, 18, 17, 12, 13, 18, 16, 15, 17,
    ];
    const REFERENCE_RMS_450PWB: [u32; 75] = [
        3, 5, 6, 10, 15, 9, 173, 1564, 761, 579, 865, 956, 1291, 1691, 1222, 362, 770, 1545, 706, 1424, 577, 23, 169, 246, 521, 1343, 1535,
        81, 27, 38, 42, 8, 219, 1363, 350, 153, 252, 1576, 1095, 216, 103, 51, 99, 484, 1536, 2141, 2332, 1807, 172, 120, 31, 14, 5, 88,
        436, 2128, 1553, 1060, 878, 36, 106, 239, 130, 13, 6, 5, 6, 6, 5, 5, 5, 8, 7, 5, 6,
    ];

    /// The loudness of each frame of `samples`.
    fn rms(samples: &[i16], frame: usize) -> Vec<f64> {
        samples.chunks(frame).map(|f| (f.iter().map(|&x| f64::from(x) * f64::from(x)).sum::<f64>() / frame as f64).sqrt()).collect()
    }

    /// How closely two loudness curves follow each other (1: exactly).
    fn correlation(a: &[f64], b: &[f64]) -> f64 {
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        let (ma, mb) = (mean(a), mean(b));
        let cov: f64 = a.iter().zip(b).map(|(x, y)| (x - ma) * (y - mb)).sum();
        let var = |v: &[f64], m: f64| v.iter().map(|x| (x - m) * (x - m)).sum::<f64>();
        cov / (var(a, ma) * var(b, mb)).sqrt()
    }

    /// A recording decoded, its sample rate, and the loudness of each 40 ms.
    fn decoded(mode: u8, bits: &[u8]) -> (u32, Vec<f64>) {
        let wav = codec2_wav(mode, bits).unwrap();
        assert!(is_wav(&wav));
        let rate = u32::from_le_bytes(wav[24..28].try_into().unwrap());
        let samples: Vec<i16> = wav[44..].chunks(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        (rate, rms(&samples, rate as usize / 25))
    }

    /// Not the same samples as the reference decoder's (unvoiced sounds get
    /// random phases), but the same sound: each frame as loud.
    fn sounds_like(ours: &[f64], reference: &[u32], loud: f64) {
        let reference: Vec<f64> = reference.iter().map(|&r| f64::from(r)).collect();
        assert_eq!(ours.len(), reference.len());
        let correlation = correlation(ours, &reference);
        assert!(correlation > 0.97, "loudness follows the reference: {correlation}");
        for (ours, theirs) in ours.iter().zip(&reference).filter(|(_, r)| **r > loud) {
            assert!((0.7..1.4).contains(&(ours / theirs)), "{ours} against {theirs}");
        }
    }

    #[test]
    fn codec2_decodes_as_the_reference_decoder_does() {
        let (rate, rms) = decoded(AM_CODEC2_1200, BITS_1200);
        assert_eq!((rate, rms.len()), (8000, 100), "100 frames of 320 samples: four seconds");
        sounds_like(&rms, &REFERENCE_RMS, 1000.0);
    }

    #[test]
    fn the_newamp_modes_decode_as_the_reference_decoder_does() {
        let (rate, rms) = decoded(AM_CODEC2_700C, BITS_700C);
        assert_eq!((rate, rms.len()), (8000, 75));
        sounds_like(&rms, &REFERENCE_RMS_700C, 1000.0);
        let (rate, rms) = decoded(AM_CODEC2_450, BITS_450);
        assert_eq!((rate, rms.len()), (8000, 75));
        sounds_like(&rms, &REFERENCE_RMS_450, 1000.0);
        // The same bits, decoded to 16 kHz.
        let (rate, rms) = decoded(AM_CODEC2_450PWB, BITS_450);
        assert_eq!((rate, rms.len()), (16000, 75));
        sounds_like(&rms, &REFERENCE_RMS_450PWB, 700.0);
    }

    #[test]
    fn seven_hundred_c_encodes_as_the_reference_encoder_does() {
        // hts1a again, as the 700C decoder makes it, encoded: not the
        // reference's frames (it encoded the original), but the same sound.
        let samples = wav_samples(&codec2_wav(AM_CODEC2_700C, BITS_700C).unwrap()).unwrap();
        let frames = codec2_encode(AM_CODEC2_700C, &samples).unwrap();
        assert_eq!(frames.len(), 75 * 4);
        let (_, again) = decoded(AM_CODEC2_700C, &frames);
        // 700C decodes a frame late (it interpolates from the last one).
        let before = rms(&samples, 320);
        let correlation = correlation(&again[1..], &before[..before.len() - 1]);
        assert!(correlation > 0.9, "loudness follows: {correlation}");
        assert!(codec2_encode(AM_CODEC2_450, &samples).is_none(), "450 isn't encoded");
    }

    #[test]
    fn what_isnt_decoded() {
        assert!(codec2_wav(AM_OPUS_OGG, b"OggS").is_none());
        assert!(codec2_wav(AM_CODEC2_1200, &[0; 5]).is_none(), "not a whole frame");
        // A part frame at the end is left out.
        let wav = codec2_wav(AM_CODEC2_3200, &[0; 8 * 3 + 5]).unwrap();
        assert_eq!(wav.len(), 44 + 3 * 160 * 2);
        assert!(codec2_wav(AM_CODEC2_2400, &[0; 6]).is_some());
    }

    #[test]
    fn send_modes_by_name() {
        assert_eq!(SEND_MODES.iter().map(|m| send_mode(m)).collect::<Vec<_>>(), [9, 8, 7, 6, 5, 4, 3]);
        assert_eq!(send_mode("nonsense"), AM_CODEC2_3200);
        assert_eq!(mode_named("Codec2 700C"), Some(AM_CODEC2_700C));
        assert_eq!(mode_named("Codec2 3200"), Some(AM_CODEC2_3200));
        assert_eq!(mode_named("Opus"), None);
    }

    #[test]
    fn recordings_encode_to_codec2_and_back() {
        // Speech: the reference recording, decoded, as a WAV file.
        let speech = codec2_wav(AM_CODEC2_1200, BITS_1200).unwrap();
        let samples = wav_samples(&speech).unwrap();
        assert_eq!(samples.len(), 32_000);
        let frames = codec2_encode(AM_CODEC2_3200, &samples).unwrap();
        // 3200: 8 bytes for each 160 samples (20 ms): four seconds, 1.6 KB.
        assert_eq!(frames.len(), 200 * 8);
        let again = wav_samples(&codec2_wav(AM_CODEC2_3200, &frames).unwrap()).unwrap();
        assert_eq!(again.len(), samples.len());
        // The same sound: the loudness of each 40 ms follows.
        let (a, b) = (rms(&samples, 320), rms(&again, 320));
        let correlation = correlation(&a, &b);
        assert!(correlation > 0.9, "loudness follows: {correlation}");
        let (ma, mb) = (a.iter().sum::<f64>(), b.iter().sum::<f64>());
        assert!((0.7..1.4).contains(&(mb / ma)), "as loud overall: {ma} and {mb}");
        // A part frame at the end is filled out.
        assert_eq!(codec2_encode(AM_CODEC2_3200, &samples[..170]).unwrap().len(), 16);
        assert!(codec2_encode(AM_CODEC2_3200, &[]).is_none());
    }

    #[test]
    fn only_8_khz_mono_16_bit_wavs_are_taken() {
        assert_eq!(wav_samples(&wav(&[1, -2, 3], 8000)).unwrap(), [1, -2, 3]);
        let mut stereo = wav(&[0; 4], 8000);
        stereo[22] = 2;
        assert!(wav_samples(&stereo).unwrap_err().contains("16-bit mono 8 kHz"));
        assert!(wav_samples(b"OggS not a wav").is_err());
    }
}
