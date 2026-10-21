/*---------------------------------------------------------------------------*\

  FILE........: newamp.rs
  AUTHOR......: David Rowe (newamp1), Thomas Kurin and Stefan Erhardt
                (newamp2); ported to Rust for rettui
  DATE CREATED: Jan 2017 (newamp1), July 2018 (newamp2)

  Quantisation functions for the sinusoidal coder, using the "newamp1"
  algorithm (700C) that resamples variable rate L [Am] to a fixed rate K
  then VQs, and "newamp2" (450, and 450PWB decoded at 16 kHz), with one VQ
  stage and plosives. A port of Codec2's newamp1.c, newamp2.c, phase.c's
  mag_to_phase() and mbest.c, as of the release before 450 was removed
  (drowe67/codec2 6549fa1~1).

\*---------------------------------------------------------------------------*/

/*
  Copyright David Rowe 2017

  All rights reserved.

  This program is free software; you can redistribute it and/or modify
  it under the terms of the GNU Lesser General Public License version 2.1, as
  published by the Free Software Foundation.  This program is
  distributed in the hope that it will be useful, but WITHOUT ANY
  WARRANTY; without even the implied warranty of MERCHANTABILITY or
  FITNESS FOR A PARTICULAR PURPOSE.  See the GNU General Public
  License for more details.

  You should have received a copy of the GNU Lesser General Public License
  along with this program; if not, see <http://www.gnu.org/licenses/>.
*/

use crate::codebooknewamp::*;
use crate::inner::*;
use crate::*;

/// rate K vector length (700C)
pub const NEWAMP1_K: usize = 20;
/// size of FFT used for phase synthesis
pub const NEWAMP1_PHASE_NFFT: usize = 128;
/// how many candidates we keep for each stage of mbest search
const NEWAMP1_VQ_MBEST_DEPTH: usize = 5;
/// rate K vector length (450)
pub const NEWAMP2_K: usize = 29;
/// rate K vector length for the 16 kHz (pseudo wideband) decoder
pub const NEWAMP2_16K_K: usize = 40;
pub const NEWAMP2_PHASE_NFFT: usize = 128;
const MBEST_STAGES: usize = 4;

/// Codec2's POW10F: 10^x by expf.
#[inline]
fn pow10f(x: f32) -> f32 {
    (2.302585092994046f32 * x).exp()
}

/*---------------------------------------------------------------------------*\

  FUNCTION....: interp_para()

  General 2nd order parabolic interpolator.  Used splines originally,
  but this is much simpler and we don't need much accuracy.  Given two
  vectors of points xp and yp, find interpolated values y at points x.

\*---------------------------------------------------------------------------*/
pub fn interp_para(y: &mut [f32], xp: &[f32], yp: &[f32], np: usize, x: &[f32], n: usize) {
    assert!(np >= 3);
    let mut k = 0;
    for i in 0..n {
        let xi = x[i];
        // k is index into xp of where we start 3 points used to form parabola
        while xp[k + 1] < xi && k < np - 3 {
            k += 1;
        }
        let (x1, y1) = (xp[k], yp[k]);
        let (x2, y2) = (xp[k + 1], yp[k + 1]);
        let (x3, y3) = (xp[k + 2], yp[k + 2]);
        let a = ((y3 - y2) / (x3 - x2) - (y2 - y1) / (x2 - x1)) / (x3 - x1);
        let b = ((y3 - y2) / (x3 - x2) * (x2 - x1) + (y2 - y1) / (x2 - x1) * (x3 - x2)) / (x3 - x1);
        y[i] = a * (xi - x2) * (xi - x2) + b * (xi - x2) + y2;
    }
}

/// Non linear sampling of frequency axis, reducing the "rate" is a first
/// step before VQ.
pub fn ftomel(f_hz: f32) -> f32 {
    (2595.0 * (1.0 + f_hz / 700.0).log10() + 0.5).floor()
}

pub fn mel_sample_freqs_khz(rate_k_sample_freqs_khz: &mut [f32], k_len: usize, mel_start: f32, mel_end: f32) {
    let step = (mel_end - mel_start) / (k_len as f32 - 1.0);
    let mut mel = mel_start;
    for k in 0..k_len {
        rate_k_sample_freqs_khz[k] = 0.7 * (pow10f(mel / 2595.0) - 1.0);
        mel += step;
    }
}

/// Fixed frequencies for the newamp2 rate K vectors, for both the 8 kHz and
/// 16 kHz decoders.
pub fn n2_mel_sample_freqs_khz(rate_k_sample_freqs_khz: &mut [f32], k_len: usize) {
    const FREQ: [f32; 40] = [
        0.199816, 0.252849, 0.309008, 0.368476, 0.431449, 0.498134, 0.568749, 0.643526, 0.722710, 0.806561, 0.895354,
        0.989380, 1.088948, 1.194384, 1.306034, 1.424264, 1.549463, 1.682041, 1.822432, 1.971098, 2.128525, 2.295232,
        2.471763, 2.658699, 2.856652, 3.066272, 3.288246, 3.523303, 3.772214, 4.035795, 4.314912, 4.610478, 4.923465,
        5.254899, 5.605865, 5.977518, 6.371075, 6.787827, 7.229141, 7.696465,
    ];
    rate_k_sample_freqs_khz[..k_len].copy_from_slice(&FREQ[..k_len]);
}

/// Resample Am from time-varying rate L=floor(pi/Wo) to fixed rate K.
pub fn resample_const_rate_f(c2const: &C2const, model: &MODEL, rate_k_vec: &mut [f32], rate_k_sample_freqs_khz: &[f32], k_len: usize) {
    let mut amdb = [0.0f32; MAX_AMP + 1];
    let mut rate_l_sample_freqs_khz = [0.0f32; MAX_AMP + 1];
    // convert rate L=pi/Wo amplitude samples to fixed rate K
    let mut amdb_peak = -100.0f32;
    for m in 1..=model.L {
        amdb[m] = 20.0 * (model.A[m] + 1E-16).log10();
        if amdb[m] > amdb_peak {
            amdb_peak = amdb[m];
        }
        rate_l_sample_freqs_khz[m] = m as f32 * model.Wo * (c2const.Fs as f32 / 2000.0) / PI;
    }
    // clip between peak and peak -50dB, to reduce dynamic range
    for m in 1..=model.L {
        if amdb[m] < amdb_peak - 50.0 {
            amdb[m] = amdb_peak - 50.0;
        }
    }
    interp_para(rate_k_vec, &rate_l_sample_freqs_khz[1..], &amdb[1..], model.L, rate_k_sample_freqs_khz, k_len);
}

/// The best few codebook entries for a target, as mbest.c keeps them.
struct Mbest {
    list: Vec<([usize; MBEST_STAGES], f32)>,
}

impl Mbest {
    fn new(entries: usize) -> Self {
        Self { list: vec![([0; MBEST_STAGES], 1E32); entries] }
    }

    fn insert(&mut self, index: &[usize; MBEST_STAGES], error: f32) {
        if let Some(i) = self.list.iter().position(|(_, e)| error < *e) {
            self.list.pop();
            self.list.insert(i, (*index, error));
        }
    }

    /// Search `cb` (`m` entries of `k`) for the closest to `vec`.
    fn search(&mut self, cb: &[f32], vec: &[f32], k: usize, m: usize, index: &mut [usize; MBEST_STAGES]) {
        for j in 0..m {
            let mut e = 0.0f32;
            for i in 0..k {
                let diff = cb[j * k + i] - vec[i];
                e += diff * diff;
            }
            index[0] = j;
            if e < self.list[self.list.len() - 1].1 {
                self.insert(index, e);
            }
        }
    }
}

/// Two stage rate K newamp1 VQ quantiser using mbest search.
fn rate_k_mbest_encode(indexes: &mut [i32], x: &[f32], xq: &mut [f32], ndim: usize, mbest_entries: usize) -> f32 {
    let codebook1 = NEWAMP1VQ_CB[0].cb;
    let codebook2 = NEWAMP1VQ_CB[1].cb;
    assert_eq!(ndim, NEWAMP1VQ_CB[0].k);
    let mut stage1 = Mbest::new(mbest_entries);
    let mut stage2 = Mbest::new(mbest_entries);
    let mut index = [0usize; MBEST_STAGES];
    let mut target = vec![0.0f32; ndim];

    // Stage 1
    stage1.search(codebook1, x, ndim, NEWAMP1VQ_CB[0].m, &mut index);
    // Stage 2
    for j in 0..mbest_entries {
        let n1 = stage1.list[j].0[0];
        index[1] = n1;
        for i in 0..ndim {
            target[i] = x[i] - codebook1[ndim * n1 + i];
        }
        stage2.search(codebook2, &target, ndim, NEWAMP1VQ_CB[1].m, &mut index);
    }
    let n1 = stage2.list[0].0[1];
    let n2 = stage2.list[0].0[0];
    let mut mse = 0.0;
    for i in 0..ndim {
        let tmp = codebook1[ndim * n1 + i] + codebook2[ndim * n2 + i];
        mse += (x[i] - tmp) * (x[i] - tmp);
        xq[i] = tmp;
    }
    indexes[0] = n1 as i32;
    indexes[1] = n2 as i32;
    mse
}

/*---------------------------------------------------------------------------*\

  FUNCTION....: post_filter

  Post Filter, has a big impact on speech quality after VQ.  When used
  on a mean removed rate K vector, it raises formants, and suppresses
  anti-formants.  As it manipulates amplitudes, we normalise energy to
  prevent clipping or large level variations.  pf_gain of 1.2 to 1.5
  (dB) seems to work OK.

\*---------------------------------------------------------------------------*/
pub fn post_filter_newamp1(vec: &mut [f32], sample_freq_khz: &[f32], k_len: usize, pf_gain: f32) {
    // vec is rate K vector describing spectrum of current frame lets pre-emp
    // before applying PF. 20dB/dec over 300Hz.  Postfilter affects energy of
    // frame so we measure energy before and after and normalise.
    let mut pre = vec![0.0f32; k_len];
    let mut e_before = 0.0f32;
    let mut e_after = 0.0f32;
    for k in 0..k_len {
        pre[k] = 20.0 * (sample_freq_khz[k] / 0.3).log10();
        vec[k] += pre[k];
        e_before += pow10f(vec[k] / 10.0);
        vec[k] *= pf_gain;
        e_after += pow10f(vec[k] / 10.0);
    }
    let gain = e_after / e_before;
    let gaindb = 10.0 * gain.log10();
    for k in 0..k_len {
        vec[k] -= gaindb;
        vec[k] -= pre[k];
    }
}

/// Decoder side interpolation of Wo and voicing, to go from 25 Hz sample
/// rate used over channel to 100Hz internal sample rate of Codec 2.
pub fn interp_wo_v(wo_: &mut [f32; 4], l_: &mut [usize; 4], voicing_: &mut [i32; 4], wo1: f32, wo2: f32, voicing1: i32, voicing2: i32) {
    const M: usize = 4;
    *voicing_ = [0; M];
    if voicing1 == 0 && voicing2 == 0 {
        *wo_ = [2.0 * PI / 100.0; M];
    }
    if voicing1 != 0 && voicing2 == 0 {
        wo_[0] = wo1;
        wo_[1] = wo1;
        wo_[2] = 2.0 * PI / 100.0;
        wo_[3] = 2.0 * PI / 100.0;
        voicing_[0] = 1;
        voicing_[1] = 1;
    }
    if voicing1 == 0 && voicing2 != 0 {
        wo_[0] = 2.0 * PI / 100.0;
        wo_[1] = 2.0 * PI / 100.0;
        wo_[2] = wo2;
        wo_[3] = wo2;
        voicing_[2] = 1;
        voicing_[3] = 1;
    }
    if voicing1 != 0 && voicing2 != 0 {
        let mut c = 1.0f32;
        for i in 0..M {
            wo_[i] = wo1 * c + wo2 * (1.0 - c);
            voicing_[i] = 1;
            c -= 1.0 / M as f32;
        }
    }
    for i in 0..M {
        l_[i] = (PI / wo_[i]).floor() as usize;
    }
}

/// Decoder side conversion of rate K vector back to rate L (newamp1), or
/// for newamp2 with plosives set to a small constant.
fn resample_rate_l_any(
    c2const: &C2const,
    model: &mut MODEL,
    rate_k_vec: &[f32],
    rate_k_sample_freqs_khz: &[f32],
    k_len: usize,
    newamp2_plosive: Option<bool>,
) {
    let mut rate_k_vec_term = vec![0.0f32; k_len + 2];
    let mut rate_k_sample_freqs_khz_term = vec![0.0f32; k_len + 2];
    let mut amdb = [0.0f32; MAX_AMP + 1];
    let mut rate_l_sample_freqs_khz = [0.0f32; MAX_AMP + 1];
    // terminate either end of the rate K vecs with 0dB points
    rate_k_sample_freqs_khz_term[k_len + 1] = 4.0;
    for k in 0..k_len {
        rate_k_vec_term[k + 1] = rate_k_vec[k];
        rate_k_sample_freqs_khz_term[k + 1] = rate_k_sample_freqs_khz[k];
    }
    for m in 1..=model.L {
        rate_l_sample_freqs_khz[m] = m as f32 * model.Wo * (c2const.Fs as f32 / 2000.0) / PI;
    }
    interp_para(&mut amdb[1..], &rate_k_sample_freqs_khz_term, &rate_k_vec_term, k_len + 2, &rate_l_sample_freqs_khz[1..], model.L);
    for m in 1..=model.L {
        model.A[m] = match newamp2_plosive {
            None => pow10f(amdb[m] / 20.0),
            Some(false) => 10.0f64.powf(amdb[m] as f64 / 20.0) as f32,
            Some(true) => 0.1,
        };
    }
}

/*---------------------------------------------------------------------------*\

  FUNCTION....: mag_to_phase

  Algorithm for http://www.dsprelated.com/showcode/20.php ported to C.
  Given a magnitude spectrum in dB, returns a minimum-phase phase spectra.

\*---------------------------------------------------------------------------*/
fn mag_to_phase(phase: &mut [f32], gdbfk: &[f32], nfft: usize, fft_fwd_cfg: &codec2_fft_cfg, fft_inv_cfg: &codec2_fft_cfg) {
    let mut sdb = vec![COMP::new(); nfft];
    let mut c = vec![COMP::new(); nfft];
    let mut cf = vec![COMP::new(); nfft];
    let mut cf_out = vec![COMP::new(); nfft];
    let ns = nfft / 2 + 1;

    // install negative frequency components, 1/Nfft takes into account kiss
    // fft lack of scaling on ifft
    sdb[0].r = gdbfk[0];
    sdb[0].i = 0.0;
    for i in 1..ns {
        sdb[i].r = gdbfk[i];
        sdb[nfft - i].r = gdbfk[i];
        sdb[i].i = 0.0;
        sdb[nfft - i].i = 0.0;
    }

    // compute real cepstrum from log magnitude spectrum
    codec2_fft(fft_inv_cfg, &sdb, &mut c);
    for i in 0..nfft {
        c[i].r /= nfft as f32;
        c[i].i /= nfft as f32;
    }

    // Fold cepstrum to reflect non-min-phase zeros inside unit circle
    cf[0] = c[0];
    for i in 1..ns - 1 {
        cf[i].r = c[i].r + c[nfft - i].r;
        cf[i].i = c[i].i + c[nfft - i].i;
    }
    cf[ns - 1] = c[ns - 1];
    // cf[ns..] stay 0

    // Cf = dB_magnitude + j * minimum_phase
    codec2_fft(fft_fwd_cfg, &cf, &mut cf_out);

    // The maths says we are meant to be using log(x), not 20*log10(x), so we
    // need to scale the phase to account for this: log(x) = 20*log10(x)/scale
    let scale = 20.0 / 10.0f32.ln();
    for i in 0..ns {
        phase[i] = cf_out[i].i / scale;
    }
}

/// Given a magnitude spectrum determine a phase spectrum, used for phase
/// synthesis with newamp1 and newamp2.
pub fn determine_phase(
    c2const: &C2const,
    h: &mut [COMP],
    model: &MODEL,
    nfft: usize,
    fwd_cfg: &codec2_fft_cfg,
    inv_cfg: &codec2_fft_cfg,
) {
    let ns = nfft / 2 + 1;
    let mut gdbfk = vec![0.0f32; ns];
    let mut sample_freqs_khz = vec![0.0f32; ns];
    let mut phase = vec![0.0f32; ns];
    let mut amdb = [0.0f32; MAX_AMP + 1];
    let mut rate_l_sample_freqs_khz = [0.0f32; MAX_AMP + 1];

    for m in 1..=model.L {
        amdb[m] = 20.0 * model.A[m].log10();
        rate_l_sample_freqs_khz[m] = m as f32 * model.Wo * (c2const.Fs as f32 / 2000.0) / PI;
    }
    for i in 0..ns {
        sample_freqs_khz[i] = (c2const.Fs as f32 / 1000.0) * i as f32 / nfft as f32;
    }
    interp_para(&mut gdbfk, &rate_l_sample_freqs_khz[1..], &amdb[1..], model.L, &sample_freqs_khz, ns);
    mag_to_phase(&mut phase, &gdbfk, nfft, fwd_cfg, inv_cfg);
    for m in 1..=model.L {
        let b = (0.5 + m as f32 * model.Wo * nfft as f32 / (2.0 * PI)).floor() as usize;
        h[m].r = phase[b].cos();
        h[m].i = phase[b].sin();
    }
}

/// Update and optionally run the "front eq" equaliser before VQ.
fn newamp1_eq(rate_k_vec_no_mean: &mut [f32], eq: &mut [f32], k_len: usize, eq_en: bool) {
    const IDEAL: [f32; 20] = [8.0, 10.0, 12.0, 14.0, 14.0, 14.0, 14.0, 14.0, 14.0, 14.0, 14.0, 14.0, 14.0, 14.0, 14.0, 14.0, 14.0, 14.0, 14.0, -20.0];
    let gain = 0.02f32;
    for k in 0..k_len {
        let update = rate_k_vec_no_mean[k] - IDEAL[k];
        eq[k] = (1.0 - gain) * eq[k] + gain * update;
        if eq[k] < 0.0 {
            eq[k] = 0.0;
        }
        if eq_en {
            rate_k_vec_no_mean[k] -= eq[k];
        }
    }
}

/// Codec2's quantise(): the nearest of `m` entries of `k` in `cb`.
fn quantise_scalar(cb: &[f32], vec: &[f32], w: &[f32], k: usize, m: usize, se: &mut f32) -> i32 {
    let mut besti = 0;
    let mut beste = 1E32f32;
    for j in 0..m {
        let mut e = 0.0f32;
        for i in 0..k {
            let diff = cb[j * k + i] - vec[i];
            e += diff * w[i] * diff * w[i];
        }
        if e < beste {
            beste = e;
            besti = j;
        }
    }
    *se += beste;
    besti as i32
}

pub fn encode_log_wo(c2const: &C2const, wo: f32, bits: u32) -> i32 {
    let wo_levels = 1i32 << bits;
    let norm = (wo.log10() - c2const.Wo_min.log10()) / (c2const.Wo_max.log10() - c2const.Wo_min.log10());
    let index = (wo_levels as f32 * norm + 0.5).floor() as i32;
    index.clamp(0, wo_levels - 1)
}

pub fn decode_log_wo(c2const: &C2const, index: i32, bits: u32) -> f32 {
    let wo_levels = 1i32 << bits;
    let step = (c2const.Wo_max.log10() - c2const.Wo_min.log10()) / wo_levels as f32;
    let wo = c2const.Wo_min.log10() + step * index as f32;
    pow10f(wo)
}

/// newamp1 encoder for amplitudes {Am}.  Given the rate L model parameters,
/// outputs VQ and energy quantiser indexes.
pub fn newamp1_model_to_indexes(
    c2const: &C2const,
    indexes: &mut [i32; 4],
    model: &MODEL,
    rate_k_sample_freqs_khz: &[f32],
    se: &mut f32,
    eq: &mut [f32],
    eq_en: bool,
) {
    let k_len = NEWAMP1_K;
    let mut rate_k_vec = [0.0f32; NEWAMP1_K];
    let mut rate_k_vec_no_mean = [0.0f32; NEWAMP1_K];
    let mut rate_k_vec_no_mean_ = [0.0f32; NEWAMP1_K];

    // convert variable rate L to fixed rate K
    resample_const_rate_f(c2const, model, &mut rate_k_vec, rate_k_sample_freqs_khz, k_len);

    // remove mean
    let sum: f32 = rate_k_vec.iter().sum();
    let mean = sum / k_len as f32;
    for k in 0..k_len {
        rate_k_vec_no_mean[k] = rate_k_vec[k] - mean;
    }

    // update and optionally run "front eq" equaliser on before VQ
    newamp1_eq(&mut rate_k_vec_no_mean, eq, k_len, eq_en);

    // two stage VQ
    rate_k_mbest_encode(indexes, &rate_k_vec_no_mean, &mut rate_k_vec_no_mean_, k_len, NEWAMP1_VQ_MBEST_DEPTH);

    // running sum of squared error for variance calculation
    for k in 0..k_len {
        *se += (rate_k_vec_no_mean[k] - rate_k_vec_no_mean_[k]).powi(2);
    }

    // scalar quantise mean (effectively the frame energy)
    let w = [1.0f32];
    let mut se_mean = 0.0f32;
    let cb = &NEWAMP1_ENERGY_CB[0];
    indexes[2] = quantise_scalar(cb.cb, &[mean], &w, cb.k, cb.m, &mut se_mean);

    // scalar quantise Wo.  We steal the smallest Wo index to signal an
    // unvoiced frame
    if model.voiced != 0 {
        let mut index = encode_log_wo(c2const, model.Wo, 6);
        if index == 0 {
            index = 1;
        }
        indexes[3] = index;
    } else {
        indexes[3] = 0;
    }
}

/// (linearly) interpolate 25Hz amplitude vectors back to 100Hz
fn interpolate(interpolated_surface_: &mut [f32], left_vec: &[f32], right_vec: &[f32], k_len: usize, plosive: bool) {
    const M: usize = 4;
    let mut c = 1.0f32;
    for i in 0..M {
        for k in 0..k_len {
            interpolated_surface_[i * k_len + k] = if !plosive {
                left_vec[k] * c + right_vec[k] * (1.0 - c)
            } else if i < 2 {
                0.0
            } else {
                right_vec[k]
            };
        }
        c -= 1.0 / M as f32;
    }
}

/// The decoder's state for newamp1 and newamp2: what's carried from one
/// 40 ms frame to the next.
#[derive(Clone, Debug)]
pub struct NewampState {
    pub rate_k_sample_freqs_khz: Vec<f32>,
    pub prev_rate_k_vec_: Vec<f32>,
    pub wo_left: f32,
    pub voicing_left: i32,
    pub phase_fft_fwd_cfg: codec2_fft_cfg,
    pub phase_fft_inv_cfg: codec2_fft_cfg,
    pub eq: Vec<f32>,
    pub eq_en: bool,
}

impl NewampState {
    /// For 700C (`k` = 20, on the mel scale from 200 Hz to 3.7 kHz).
    pub fn newamp1() -> Self {
        let mut freqs = vec![0.0; NEWAMP1_K];
        mel_sample_freqs_khz(&mut freqs, NEWAMP1_K, ftomel(200.0), ftomel(3700.0));
        Self::with(freqs, NEWAMP1_PHASE_NFFT)
    }

    /// For 450 (`k` = 29) or 450PWB (`k` = 40).
    pub fn newamp2(k_len: usize) -> Self {
        let mut freqs = vec![0.0; k_len];
        n2_mel_sample_freqs_khz(&mut freqs, k_len);
        Self::with(freqs, NEWAMP2_PHASE_NFFT)
    }

    fn with(freqs: Vec<f32>, nfft: usize) -> Self {
        let k_len = freqs.len();
        Self {
            rate_k_sample_freqs_khz: freqs,
            prev_rate_k_vec_: vec![0.0; k_len],
            wo_left: 0.0,
            voicing_left: 0,
            phase_fft_fwd_cfg: kiss_fft_state::new(nfft, 0),
            phase_fft_inv_cfg: kiss_fft_state::new(nfft, 1),
            eq: vec![0.0; k_len],
            eq_en: false,
        }
    }
}

/// newamp1 decoder: four 10 ms models, and their phase spectra `h`
/// (`MAX_AMP + 1` each), from a 700C frame's indexes.
pub fn newamp1_indexes_to_model(
    c2const: &C2const,
    model_: &mut [MODEL; 4],
    h: &mut [COMP],
    state: &mut NewampState,
    indexes: &[i32; 4],
    post_filter_en: bool,
) {
    let k_len = NEWAMP1_K;
    let codebook1 = NEWAMP1VQ_CB[0].cb;
    let codebook2 = NEWAMP1VQ_CB[1].cb;
    let (n1, n2) = (indexes[0] as usize, indexes[1] as usize);
    let mut rate_k_vec_no_mean_ = [0.0f32; NEWAMP1_K];
    for k in 0..k_len {
        rate_k_vec_no_mean_[k] = codebook1[k_len * n1 + k] + codebook2[k_len * n2 + k];
    }
    if post_filter_en {
        post_filter_newamp1(&mut rate_k_vec_no_mean_, &state.rate_k_sample_freqs_khz, k_len, 1.5);
    }
    let mean_ = NEWAMP1_ENERGY_CB[0].cb[indexes[2] as usize];
    let mut rate_k_vec_ = [0.0f32; NEWAMP1_K];
    for k in 0..k_len {
        rate_k_vec_[k] = rate_k_vec_no_mean_[k] + mean_;
    }

    // decode latest Wo and voicing
    let (wo_right, voicing_right) = if indexes[3] != 0 { (decode_log_wo(c2const, indexes[3], 6), 1) } else { (2.0 * PI / 100.0, 0) };

    to_models(c2const, model_, h, state, &rate_k_vec_, wo_right, voicing_right, false, None, NEWAMP1_PHASE_NFFT);
}

/// newamp2 decoder: as [`newamp1_indexes_to_model`], for 450 (`wideband`
/// false, 8 kHz) or 450PWB (16 kHz).
pub fn newamp2_indexes_to_model(
    c2const: &C2const,
    model_: &mut [MODEL; 4],
    h: &mut [COMP],
    state: &mut NewampState,
    indexes: &[i32; 4],
    pf_gain: f32,
    wideband: bool,
) {
    let k_len = state.rate_k_sample_freqs_khz.len();
    let codebook1 = NEWAMP2VQ_CB[0].cb;
    let n1 = indexes[0] as usize;
    let stride = NEWAMP2_16K_K + 1;
    let mut rate_k_vec_no_mean_ = vec![0.0f32; k_len];
    for k in 0..k_len {
        rate_k_vec_no_mean_[k] = codebook1[stride * n1 + k];
    }
    let mean_ = NEWAMP2_ENERGY_CB[0].cb[indexes[2] as usize];
    let mut rate_k_vec_ = vec![0.0f32; k_len];
    post_filter_newamp1(&mut rate_k_vec_no_mean_, &state.rate_k_sample_freqs_khz, k_len, pf_gain);
    if !wideband {
        for k in 0..k_len {
            rate_k_vec_[k] = rate_k_vec_no_mean_[k] + mean_;
        }
    } else {
        // Extends the sample rate by looking up the corresponding higher
        // frequency values with their energy difference to the base energy
        let mean2 = (mean_ + codebook1[stride * n1 + k_len] - 10.0).min(50.0);
        for k in 0..k_len {
            rate_k_vec_[k] = rate_k_vec_no_mean_[k] + if k < NEWAMP2_K { mean_ } else { mean2 };
        }
    }

    // decode latest Wo and voicing and plosive
    let (wo_right, voicing_right, plosive) = match indexes[3] {
        // Voiced with Wo
        1..=62 => (decode_log_wo(c2const, indexes[3], 6), 1, false),
        // Unvoiced
        0 => (2.0 * PI / 100.0, 0, false),
        // 63: plosive (and unvoiced)
        _ => (2.0 * PI / 100.0, 0, true),
    };
    to_models(c2const, model_, h, state, &rate_k_vec_, wo_right, voicing_right, plosive, Some(()), NEWAMP2_PHASE_NFFT);
}

/// The rest of both decoders: interpolate from the last frame to this one,
/// back to rate L amplitudes, and a phase for each of the four.
#[allow(clippy::too_many_arguments)]
fn to_models(
    c2const: &C2const,
    model_: &mut [MODEL; 4],
    h: &mut [COMP],
    state: &mut NewampState,
    rate_k_vec_: &[f32],
    wo_right: f32,
    voicing_right: i32,
    plosive: bool,
    newamp2: Option<()>,
    nfft: usize,
) {
    const M: usize = 4;
    let k_len = state.rate_k_sample_freqs_khz.len();
    // interpolate 25Hz rate K vec back to 100Hz
    let mut interpolated_surface_ = vec![0.0f32; M * k_len];
    interpolate(&mut interpolated_surface_, &state.prev_rate_k_vec_, rate_k_vec_, k_len, plosive);

    // interpolate 25Hz v and Wo back to 100Hz
    let mut awo_ = [0.0f32; M];
    let mut al_ = [0usize; M];
    let mut avoicing_ = [0i32; M];
    interp_wo_v(&mut awo_, &mut al_, &mut avoicing_, state.wo_left, wo_right, state.voicing_left, voicing_right);

    // back to rate L amplitudes, synthesise phase for each frame
    for i in 0..M {
        model_[i].Wo = awo_[i];
        model_[i].L = al_[i];
        model_[i].voiced = avoicing_[i];
        let surface = &interpolated_surface_[k_len * i..k_len * (i + 1)];
        let plosive_here = newamp2.map(|()| plosive && i < 2);
        resample_rate_l_any(c2const, &mut model_[i], surface, &state.rate_k_sample_freqs_khz, k_len, plosive_here);
        determine_phase(
            c2const,
            &mut h[(MAX_AMP + 1) * i..(MAX_AMP + 1) * (i + 1)],
            &model_[i],
            nfft,
            &state.phase_fft_fwd_cfg,
            &state.phase_fft_inv_cfg,
        );
    }

    // update memories for next time
    state.prev_rate_k_vec_.copy_from_slice(rate_k_vec_);
    state.wo_left = wo_right;
    state.voicing_left = voicing_right;
}
