//! RtStore — feedback del hilo de audio hacia la GUI y los LEDs, solo atomics
//! (mismo patrón que store.rs de Caja: AtomicU32 con bits de f32 para picos).
//! El audio ESCRIBE, la GUI/LEDs LEEN. Nunca hay locks aquí.
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use lanzadera_core::{ClipState, MAX_SCENES, MAX_TRACKS};

pub fn encode_state(s: ClipState) -> u8 {
    match s {
        ClipState::Empty => 0, ClipState::Triggered => 1, ClipState::Playing => 2,
        ClipState::Stopping => 3, ClipState::Recording => 4, ClipState::Overdubbing => 5,
        ClipState::RecQueued => 6,
    }
}
pub fn decode_state(v: u8) -> ClipState {
    match v {
        1 => ClipState::Triggered, 2 => ClipState::Playing, 3 => ClipState::Stopping,
        4 => ClipState::Recording, 5 => ClipState::Overdubbing, 6 => ClipState::RecQueued,
        _ => ClipState::Empty,
    }
}

pub struct RtStore {
    clip_state:   [AtomicU8;  MAX_TRACKS * MAX_SCENES],
    playhead:     [AtomicU32; MAX_TRACKS], // ticks del clip activo
    active_scene: [AtomicU8;  MAX_TRACKS], // 0xFF = sin clip activo
    peak:         [AtomicU32; MAX_TRACKS + 1], // f32 bits; último = master
    groove_step:    AtomicU8,  // step actual del motor de Caja (0..15, 255=parado)
}

impl RtStore {
    pub fn new() -> Self {
        RtStore {
            clip_state:   std::array::from_fn(|_| AtomicU8::new(0)),
            playhead:     std::array::from_fn(|_| AtomicU32::new(0)),
            active_scene: std::array::from_fn(|_| AtomicU8::new(0xFF)),
            peak:         std::array::from_fn(|_| AtomicU32::new(0)),
            groove_step:    AtomicU8::new(255),
        }
    }
    pub fn set_clip_state(&self, t: usize, s: usize, st: ClipState) {
        self.clip_state[t * MAX_SCENES + s].store(encode_state(st), Ordering::Relaxed);
    }
    pub fn clip_state(&self, t: usize, s: usize) -> ClipState {
        decode_state(self.clip_state[t * MAX_SCENES + s].load(Ordering::Relaxed))
    }
    pub fn set_playhead(&self, t: usize, ticks: u32) { self.playhead[t].store(ticks, Ordering::Relaxed); }
    pub fn playhead(&self, t: usize) -> u32 { self.playhead[t].load(Ordering::Relaxed) }
    pub fn set_active_scene(&self, t: usize, s: u8) { self.active_scene[t].store(s, Ordering::Relaxed); }
    pub fn active_scene(&self, t: usize) -> Option<u8> {
        let v = self.active_scene[t].load(Ordering::Relaxed);
        if v == 0xFF { None } else { Some(v) }
    }
    pub fn set_peak(&self, t: usize, v: f32) { self.peak[t].store(v.to_bits(), Ordering::Relaxed); }
    pub fn peak(&self, t: usize) -> f32 { f32::from_bits(self.peak[t].load(Ordering::Relaxed)) }
    pub fn set_groove_step(&self, step: u8) { self.groove_step.store(step, Ordering::Relaxed); }
    pub fn groove_step(&self) -> u8 { self.groove_step.load(Ordering::Relaxed) }
}

#[cfg(test)]
mod rec_queued_rt_tests {
    use super::*;

    #[test]
    fn roundtrip_de_todos_los_estados() {
        for s in [
            ClipState::Empty, ClipState::Triggered, ClipState::Playing,
            ClipState::Stopping, ClipState::Recording, ClipState::Overdubbing,
            ClipState::RecQueued,
        ] {
            assert_eq!(decode_state(encode_state(s)), s);
        }
    }
}
