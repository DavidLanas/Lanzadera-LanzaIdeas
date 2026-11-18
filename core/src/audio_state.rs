//! SessionAudio — snapshot POD para el hilo de audio (idea de KitAudio de Caja).
//! Sin String, sin Vec: arrays fijos copiables por TripleBuffer.
//! Extensión de Fase 1: tabla ClipMeta (quantize/launch/follow/probability por
//! clip) para que el ClipEngine no necesite tocar SessionState ni hacer lock.
use crate::{ClipState, FollowAction, LaunchMode, Quantize, SessionState, MAX_SCENES, MAX_TRACKS, TICKS_PER_BAR};

#[derive(Clone, Copy, Debug)]
pub struct ClipAudioRef {
    pub track_idx: u8,
    pub scene_idx: u8,
    pub state: ClipState,
    pub playhead: u32, // ticks (480 PPQ)
    pub loop_ticks: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct TrackAudio {
    pub volume: f32,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    pub eq_low: f32,
    pub eq_mid: f32,
    pub eq_high: f32,
    pub hpf_on: bool,
    pub lpf_on: bool,
    pub hpf_hz: f32,
    pub lpf_hz: f32,
    pub sends: [f32; 4],
    pub active_clip: Option<ClipAudioRef>,
}

impl TrackAudio {
    pub const EMPTY: Self = TrackAudio {
        volume: 0.0, pan: 0.0, mute: false, solo: false,
        eq_low: 0.0, eq_mid: 0.0, eq_high: 0.0, hpf_on: false, lpf_on: false,
        hpf_hz: 20.0, lpf_hz: 20000.0, sends: [0.0; 4], active_clip: None,
    };
}
impl Default for TrackAudio { fn default() -> Self { Self::EMPTY } }

/// Metadatos POD de un clip (lo que el motor necesita para decidir sin locks).
#[derive(Clone, Copy, Debug)]
pub struct ClipMeta {
    pub exists: bool,
    pub launch_mode: LaunchMode,
    pub quantize: Quantize,
    pub follow_action: FollowAction,
    pub follow_bars: u8,
    pub loop_bars: u8,
    pub loop_count: u8, // 0 = infinito
    pub probability: f32,
}

impl ClipMeta {
    pub const EMPTY: Self = ClipMeta {
        exists: false, launch_mode: LaunchMode::Trigger, quantize: Quantize::One,
        follow_action: FollowAction::None, follow_bars: 0, loop_bars: 1,
        loop_count: 0, probability: 1.0,
    };
}
impl Default for ClipMeta { fn default() -> Self { Self::EMPTY } }

#[derive(Clone, Copy)]
pub struct SessionAudio {
    pub tracks: [TrackAudio; MAX_TRACKS],
    pub track_count: u8,
    pub any_solo: bool,
    pub tempo: f64,
    pub playing: bool,
    pub master_volume: f32,
    pub quantize: Quantize, // global (stops del launcher)
    pub fill_mode: bool,    // hook Fase 4 (occurrence de Caja)
    pub recording: bool,
    pub rec_track: u8,
    pub rec_scene: u8,
    pub rec_loop_ticks: u32,
    /// Rejilla de la grabación en ticks (0 = Immediate).
    pub rec_q_ticks: u32,
    /// Longitud del loop de Groove Machine en ticks (0 = apagado / desconocido).
    /// El launcher alinea el quantize de lanzamiento a este ciclo para que
    /// clips y batería compartan el mismo downbeat de frase.
    pub groove_loop_ticks: u32,
    pub clips: [ClipMeta; MAX_TRACKS * MAX_SCENES], // ~12 KB: memcpy trivial por callback
}

impl Default for SessionAudio {
    fn default() -> Self {
        SessionAudio {
            tracks: [TrackAudio::EMPTY; MAX_TRACKS], track_count: 0, any_solo: false,
            tempo: 120.0, playing: false, master_volume: 0.8,
            quantize: Quantize::One, fill_mode: false,
            recording: false, rec_track: 0, rec_scene: 0, rec_loop_ticks: 3840,
            rec_q_ticks: 0,
            groove_loop_ticks: 0,
            clips: [ClipMeta::EMPTY; MAX_TRACKS * MAX_SCENES],
        }
    }
}

impl SessionAudio {
    pub fn from_state(s: &SessionState) -> Self {
        let (rt, rs, rl) = s.record_target();
        let mut out = SessionAudio {
            track_count: s.tracks.len().min(MAX_TRACKS) as u8,
            recording: s.recording, rec_track: rt as u8, rec_scene: rs as u8, rec_loop_ticks: rl,
            rec_q_ticks: s.quantize.ticks() as u32,
            tempo: s.tempo, playing: s.playing, master_volume: s.master_volume,
            quantize: s.quantize, fill_mode: s.fill_mode,
            ..Default::default()
        };
        for (ti, tr) in s.tracks.iter().enumerate().take(MAX_TRACKS) {
            let mut active = None;
            for (si, slot) in tr.clips.iter().enumerate() {
                if let Some(c) = slot {
                    out.clips[ti * MAX_SCENES + si] = ClipMeta {
                        exists: true, launch_mode: c.launch_mode, quantize: c.quantize,
                        follow_action: c.follow_action, follow_bars: c.follow_bars,
                        loop_bars: c.loop_bars.max(1), loop_count: c.loop_count,
                        probability: c.probability,
                    };
                    if active.is_none()
                        && matches!(c.state, ClipState::Playing | ClipState::Triggered | ClipState::Recording | ClipState::Overdubbing)
                    {
                        active = Some(ClipAudioRef {
                            track_idx: ti as u8, scene_idx: si as u8, state: c.state,
                            playhead: 0, loop_ticks: c.loop_bars.max(1) as u32 * TICKS_PER_BAR as u32,
                        });
                    }
                }
            }
            out.tracks[ti] = TrackAudio {
                volume: tr.volume, pan: tr.pan, mute: tr.mute, solo: tr.solo,
                eq_low: tr.eq_low, eq_mid: tr.eq_mid, eq_high: tr.eq_high,
                hpf_on: tr.hpf_on, lpf_on: tr.lpf_on, hpf_hz: tr.hpf_hz, lpf_hz: tr.lpf_hz,
                sends: tr.sends, active_clip: active,
            };
            out.any_solo |= tr.solo;
        }
        out
    }

    #[inline]
    pub fn clip_meta(&self, t: usize, s: usize) -> ClipMeta {
        self.clips[t * MAX_SCENES + s]
    }
}
