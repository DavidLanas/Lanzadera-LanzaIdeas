// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! Sinte interno de referencia (Fase 3A): polifónico, saw+sub, ADSR y un
//! lowpass de un polo por voz. Existe para que los clips MIDI suenen sin
//! plugins; en Fase 3B el host CLAP se suma por pista y esto queda de fallback.
//! Sin alloc: banco fijo de 64 voces con robo de la más antigua.
use lanzadera_core::MAX_TRACKS;

#[derive(Clone, Copy)]
struct Voice {
    active: bool, gate: bool, track: u8, note: u8,
    phase: f32, freq: f32, env: f32, amp: f32, lp: f32, age: u32,
}
const EMPTY: Voice = Voice { active: false, gate: false, track: 0, note: 0, phase: 0.0, freq: 0.0, env: 0.0, amp: 0.0, lp: 0.0, age: 0 };

pub struct Synth {
    voices: [Voice; 64],
    sums: [f32; MAX_TRACKS],
    sr: f32,
    attack: f32,
    decay: f32,
    release: f32,
    sustain: f32,
    age: u32,
    bend_mult: [f32; MAX_TRACKS],
    modw: [f32; MAX_TRACKS],
}

impl Synth {
    pub fn new(sr: f32) -> Self {
        Synth {
            voices: [EMPTY; 64], sums: [0.0; MAX_TRACKS], sr: sr.max(1.0),
            attack: 1.0 / (0.005 * sr), decay: 1.0 / (0.12 * sr),
            release: 1.0 / (0.12 * sr), sustain: 0.65, age: 0,
            bend_mult: [1.0; MAX_TRACKS], modw: [0.0; MAX_TRACKS],
        }
    }

    pub fn note_on(&mut self, track: u8, note: u8, vel: u8) {
        let idx = match self.voices.iter().position(|v| !v.active) {
            Some(i) => i,
            None => self.voices.iter().enumerate().max_by_key(|(_, v)| v.age).map(|(i, _)| i).unwrap_or(0),
        };
        self.age = self.age.wrapping_add(1);
        let freq = 440.0 * ((note as f32 - 69.0) / 12.0).exp2();
        self.voices[idx] = Voice {
            active: true, gate: true, track, note, phase: 0.0, freq,
            env: 0.0, amp: (vel as f32 / 127.0).max(0.05), lp: 0.0, age: self.age,
        };
    }

    pub fn note_off(&mut self, track: u8, note: u8) {
        for v in self.voices.iter_mut() {
            if v.active && v.track == track && v.note == note { v.gate = false; }
        }
    }

    /// Pitch bend (14 bits, 8192 = centro) → ±2 semitonos. Se precalcula el
    /// multiplicador para no hacer powf por muestra.
    pub fn pitch_bend(&mut self, track: u8, value: u16) {
        let t = (track as usize).min(MAX_TRACKS - 1);
        let semis = ((value as f32 / 8192.0) - 1.0) * 2.0;
        self.bend_mult[t] = (2.0f32).powf(semis / 12.0);
    }

    /// Rueda de modulación (CC1) → abre el lowpass (sonido más brillante).
    pub fn mod_wheel(&mut self, track: u8, value: u8) {
        let t = (track as usize).min(MAX_TRACKS - 1);
        self.modw[t] = value as f32 / 127.0;
    }

    pub fn all_off(&mut self, track: u8) {
        for v in self.voices.iter_mut() { if v.track == track { v.gate = false; } }
    }

    /// Avanza un sample de todas las voces y acumula por pista.
    pub fn advance(&mut self) {
        self.sums = [0.0; MAX_TRACKS];
        for v in self.voices.iter_mut() {
            if !v.active { continue; }
            if v.gate {
                if v.env < 1.0 { v.env = (v.env + self.attack).min(1.0); }
                else { v.env += (self.sustain - v.env) * self.decay; }
            } else {
                v.env -= self.release;
                if v.env <= 0.0 { v.active = false; v.env = 0.0; continue; }
            }
            let t = (v.track as usize).min(MAX_TRACKS - 1);
            v.phase += (v.freq * self.bend_mult[t]) / self.sr;
            if v.phase >= 1.0 { v.phase -= 1.0; }
            let saw = 2.0 * v.phase - 1.0;
            let sub = if v.phase < 0.5 { 0.5 } else { -0.5 };
            let raw = saw * 0.7 + sub * 0.3;
            v.lp += (raw - v.lp) * (0.08 + 0.25 * v.env + 0.5 * self.modw[t]);
            let out = v.lp * v.env * v.amp * 0.22;
            self.sums[(v.track as usize).min(MAX_TRACKS - 1)] += out;
        }
    }

    #[inline]
    pub fn track_sum(&self, t: usize) -> f32 { self.sums[t.min(MAX_TRACKS - 1)] }
}
