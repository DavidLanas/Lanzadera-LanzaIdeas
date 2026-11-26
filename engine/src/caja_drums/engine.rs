// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! Wrapper del Engine de Caja para Lanzadera.
//! Adaptado de plugin/src/dsp.rs, pero usando solo tipos de caja-core.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use caja_core::kit::{is_lock, resolve_hit, resolve_hit_hold, PadLive, PadRuntime, SamplePick, NUM_PADS, P_SAMPLE, SLOTS};
use caja_core::seedrand::RollRng;

use crate::caja_drums::sample::Sample;
use crate::caja_drums::store::CajaStore;

/// Voces físicas por pad: hasta 4 sonando + una más para el fundido de la que se roba.
pub const VOICES_PER_PAD: usize = 5;
const FADE_MS: f32 = 4.0;

// ───────────────────────────── conversiones de valor 0..1 ─────────────────────────────

pub fn pitch_semitones(v: f32) -> f32 { (v * 2.0 - 1.0) * 24.0 }
pub fn decay_seconds(v: f32) -> Option<f32> { (v < 0.999).then(|| 0.01 * 600.0f32.powf(v)) }
pub fn cutoff_hz(v: f32) -> Option<f32> { (v < 0.999).then(|| 100.0 * 200.0f32.powf(v)) }

// ───────────────────────────── voz ─────────────────────────────

#[derive(Clone)]
struct Voice {
    active: bool,
    fading: bool,
    sample: Option<Arc<Sample>>,
    pos: f64,
    rate: f64,
    env: f32,
    env_mul: f32,
    gl: f32,
    gr: f32,
    lp: bool,
    a1: f32,
    a2: f32,
    a3: f32,
    ic1: [f32; 2],
    ic2: [f32; 2],
    age: u64,
}

impl Voice {
    const fn idle() -> Voice {
        Voice {
            active: false, fading: false, sample: None, pos: 0.0, rate: 1.0,
            env: 1.0, env_mul: 1.0, gl: 0.0, gr: 0.0, lp: false,
            a1: 0.0, a2: 0.0, a3: 0.0, ic1: [0.0; 2], ic2: [0.0; 2], age: 0,
        }
    }
}

#[inline]
fn cubic(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * t + c2) * t + c1) * t + y1
}

#[inline]
fn tap(v: &[f32], i: usize, t: f32) -> f32 {
    let n = v.len();
    if i >= 1 && i + 2 < n {
        cubic(v[i - 1], v[i], v[i + 1], v[i + 2], t)
    } else {
        let g = |j: isize| if j < 0 || j as usize >= n { 0.0 } else { v[j as usize] };
        let i = i as isize;
        cubic(g(i - 1), g(i), g(i + 1), g(i + 2), t)
    }
}

#[inline]
fn svf(ic1: &mut f32, ic2: &mut f32, x: f32, a1: f32, a2: f32, a3: f32) -> f32 {
    let v3 = x - *ic2;
    let v1 = a1 * *ic1 + a2 * v3;
    let v2 = *ic2 + a2 * *ic1 + a3 * v3;
    *ic1 = 2.0 * v1 - *ic1;
    *ic2 = 2.0 * v2 - *ic2;
    v2
}

pub struct CajaEngine {
    voices: Vec<Voice>,
    sr: f32,
    clock: u64,
    rt: [PadRuntime; NUM_PADS],
    eff: [PadLive; NUM_PADS],
    prev_param: [Option<(u32, u8)>; NUM_PADS],
    last: [Option<Arc<Sample>>; NUM_PADS],
    pub roll: RollRng,
    fade_mul: f32,
    pub pin_missing_hits: u32,
    mute: [bool; NUM_PADS],
    solo: [bool; NUM_PADS],
    any_solo: bool,
    pan: [f32; NUM_PADS],
    gain: [f32; NUM_PADS],
    rate: [u8; NUM_PADS],
    last_roll_bar: [i64; NUM_PADS],
}

impl CajaEngine {
    pub fn new(sr: f32) -> Self {
        let mut e = CajaEngine {
            voices: vec![Voice::idle(); NUM_PADS * VOICES_PER_PAD],
            sr: sr.max(8000.0),
            clock: 0,
            rt: std::array::from_fn(|i| PadRuntime::new(PadLive::default_for(i).seed)),
            eff: std::array::from_fn(PadLive::default_for),
            prev_param: [None; NUM_PADS],
            last: std::array::from_fn(|_| None),
            roll: RollRng::new(0x5EED_CA7A),
            fade_mul: 1.0,
            pin_missing_hits: 0,
            mute: [false; NUM_PADS],
            solo: [false; NUM_PADS],
            any_solo: false,
            pan: [0.0; NUM_PADS],
            gain: [1.0; NUM_PADS],
            rate: [0; NUM_PADS],
            last_roll_bar: [i64::MIN; NUM_PADS],
        };
        e.set_sample_rate(sr);
        e
    }

    pub fn set_sample_rate(&mut self, sr: f32) {
        self.sr = sr.max(8000.0);
        self.fade_mul = (-6.9078 / (FADE_MS * 0.001 * self.sr)).exp();
    }

    pub fn reset(&mut self) {
        for v in &mut self.voices {
            v.active = false;
            v.sample = None;
        }
        self.last = std::array::from_fn(|_| None);
    }

    pub fn reseed_roll(&mut self, seed: u64) {
        self.roll = RollRng::new(seed);
    }

    pub fn begin_block(&mut self, pads: &[caja_core::kit::PadCfg], host: &[(u32, u8, u8); NUM_PADS]) {
        for (pad, &(seed, slot, mode)) in host.iter().enumerate() {
            let slot = slot.min(SLOTS as u8 - 1);
            match self.prev_param[pad] {
                None => self.eff[pad].seed = seed,
                Some((ps, pslot)) => {
                    if seed != ps {
                        self.eff[pad].seed = seed;
                    } else if slot != pslot {
                        self.eff[pad].seed = pads[pad].bank[usize::from(slot)];
                    }
                }
            }
            self.prev_param[pad] = Some((seed, slot));
            self.eff[pad].slot = slot;
            self.eff[pad].mode = mode & 0b1111;
        }
    }

    pub fn set_mixer(
        &mut self,
        mute: &[bool; NUM_PADS],
        solo: &[bool; NUM_PADS],
        pan: &[f32; NUM_PADS],
        gain: &[f32; NUM_PADS],
        rate: &[u8; NUM_PADS],
    ) {
        self.mute = *mute;
        self.solo = *solo;
        self.any_solo = solo.iter().any(|&s| s);
        self.pan = *pan;
        self.gain = *gain;
        self.rate = *rate;
    }

    fn voice_range(pad: usize) -> std::ops::Range<usize> {
        pad * VOICES_PER_PAD..(pad + 1) * VOICES_PER_PAD
    }

    fn start_fade(v: &mut Voice, mul: f32) {
        if v.active {
            v.fading = true;
            v.env_mul = mul;
        }
    }

    pub fn trigger(&mut self, pad: usize, vel: f32, pads: &[caja_core::kit::PadCfg], store: &CajaStore, song_beats: f64) {
        if pad >= NUM_PADS { return; }
        let cfg = &pads[pad];
        let pad_solo = self.solo[pad] || cfg.solo;
        let any_solo = self.any_solo || pads.iter().any(|p| p.solo);
        if self.mute[pad] || (any_solo && !pad_solo) { return; }
        self.clock += 1;
        let live = self.eff[pad];
        let n = store.bank_len[pad].load(Ordering::Relaxed) as usize;

        let rate = self.rate[pad];
        let hp = if rate == 0 {
            // "Al toque": re-tira en cada golpe.
            resolve_hit(&cfg.cfg, live, &mut self.rt[pad], &mut self.roll, n)
        } else if song_beats >= 0.0 {
            let bars_per = match rate { 1 => 1i64, 2 => 2, 3 => 3, _ => 4 };
            let bar = (song_beats / 4.0).floor() as i64;
            let due = self.last_roll_bar[pad] == i64::MIN || bar - self.last_roll_bar[pad] >= bars_per;
            if due {
                self.last_roll_bar[pad] = bar;
                resolve_hit(&cfg.cfg, live, &mut self.rt[pad], &mut self.roll, n)
            } else {
                resolve_hit_hold(&cfg.cfg, live, &self.rt[pad], n)
            }
        } else {
            resolve_hit_hold(&cfg.cfg, live, &self.rt[pad], n)
        };

        let pin = store.pin_idx[pad].load(Ordering::Relaxed);
        let locked = is_lock(live.mode, P_SAMPLE);
        let pick = if locked && pin >= 0 {
            SamplePick { index: Some(pin as usize), pin_missing: false }
        } else {
            let missing = locked && store.pin_missing[pad].load(Ordering::Relaxed);
            if missing { self.pin_missing_hits = self.pin_missing_hits.wrapping_add(1); }
            SamplePick { index: hp.index, pin_missing: missing }
        };

        let sample = match pick.index {
            Some(i) if i < n => {
                let slot = CajaStore::slot(pad, i);
                store.cur_idx[pad].store(i as i32, Ordering::Relaxed);
                store.last_use[slot].store(store.clock.load(Ordering::Relaxed), Ordering::Relaxed);
                match store.load_sample(pad, i) {
                    Some(s) => {
                        self.last[pad] = Some(s.clone());
                        Some(s.clone())
                    }
                    None => {
                        if !store.requested[slot].swap(true, Ordering::Relaxed) {
                            let _ = store.want.push(((pad as u32) << 8) | i as u32);
                        }
                        self.last[pad].clone()
                    }
                }
            }
            _ => self.last[pad].clone(),
        };
        let Some(sample) = sample else { return };
        if sample.is_empty() { return; }

        if cfg.choke_group != 0 && cfg.choker {
            for q in 0..NUM_PADS {
                if q != pad && pads[q].choke_group == cfg.choke_group {
                    let fm = self.fade_mul;
                    for v in &mut self.voices[Self::voice_range(q)] {
                        Self::start_fade(v, fm);
                    }
                }
            }
        }

        let poly = usize::from(cfg.poly.clamp(1, 4));
        let range = Self::voice_range(pad);
        let fm = self.fade_mul;
        let live_count = self.voices[range.clone()].iter().filter(|v| v.active && !v.fading).count();
        if live_count >= poly {
            if let Some(oldest) = self.voices[range.clone()].iter_mut().filter(|v| v.active && !v.fading).min_by_key(|v| v.age) {
                Self::start_fade(oldest, fm);
            }
        }
        let idx = self.voices[range.clone()]
            .iter()
            .position(|v| !v.active)
            .map(|i| range.start + i)
            .unwrap_or_else(|| range.start + self.voices[range.clone()].iter().enumerate().min_by_key(|(_, v)| v.age).map_or(0, |(i, _)| i));

        let rate = f64::from((pitch_semitones(hp.pitch) / 12.0).exp2());
        let env_mul = match decay_seconds(hp.decay) {
            Some(d) => (-6.9078 / (d * self.sr)).exp(),
            None => 1.0,
        };
        let g = self.gain[pad].clamp(0.0, 2.0);
        let gain = g * vel.clamp(0.0, 1.0);
        let pan = self.pan[pad].clamp(-1.0, 1.0);
        let ang = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
        let (gl, gr) = (ang.cos() * std::f32::consts::SQRT_2 * gain, ang.sin() * std::f32::consts::SQRT_2 * gain);
        let v = &mut self.voices[idx];
        *v = Voice::idle();
        v.active = true;
        v.sample = Some(sample);
        v.rate = rate;
        v.env_mul = env_mul;
        v.gl = gl;
        v.gr = gr;
        v.age = self.clock;
        if let Some(fc) = cutoff_hz(hp.filter) {
            let fc = fc.min(self.sr * 0.45);
            let g = (std::f32::consts::PI * fc / self.sr).tan();
            v.lp = true;
            v.a1 = 1.0 / (1.0 + g * (g + 1.4142));
            v.a2 = g * v.a1;
            v.a3 = g * v.a2;
        }
    }

    pub fn render(&mut self, l: &mut [f32], r: &mut [f32]) {
        let frames = l.len().min(r.len());
        for v in &mut self.voices {
            if !v.active { continue; }
            let Some(s) = v.sample.take() else {
                v.active = false;
                continue;
            };
            let len = s.left.len();
            let stereo = s.right.is_some();
            let right = s.right.as_deref().unwrap_or(&s.left);
            for i in 0..frames {
                let ip = v.pos as usize;
                if ip >= len {
                    v.active = false;
                    break;
                }
                let t = (v.pos - ip as f64) as f32;
                let mut xl = tap(&s.left, ip, t);
                let mut xr = if stereo { tap(right, ip, t) } else { xl };
                if v.lp {
                    xl = svf(&mut v.ic1[0], &mut v.ic2[0], xl, v.a1, v.a2, v.a3);
                    xr = if stereo { svf(&mut v.ic1[1], &mut v.ic2[1], xr, v.a1, v.a2, v.a3) } else { xl };
                }
                l[i] += xl * v.env * v.gl;
                r[i] += xr * v.env * v.gr;
                v.env *= v.env_mul;
                v.pos += v.rate;
                if v.env < 1.0e-4 && (v.fading || v.env_mul < 1.0) {
                    v.active = false;
                    break;
                }
            }
            if v.active { v.sample = Some(s); }
        }
    }
}
