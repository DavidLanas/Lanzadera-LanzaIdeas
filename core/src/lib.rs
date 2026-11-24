//! lanzadera-core — modelo de datos puro del clip launcher.
//! Sin audio ni UI: estado, comandos y primitivas de concurrencia
//! (TripleBuffer / Spsc) portadas textualmente de caja-core.

pub mod clip;
pub mod track;
pub mod scene;
pub mod session;
pub mod audio_state;
pub mod control;
pub mod schedule;
pub mod spsc;
pub mod json;

pub use clip::{Clip, ClipState, FollowAction, LaunchMode, MidiNote, Quantize};
pub use track::{PluginKind, Track, TrackKind};
pub use scene::Scene;
pub use session::{SessionState, FORMAT_VERSION};
pub use audio_state::{ClipAudioRef, ClipMeta, SessionAudio, TrackAudio};
pub use control::{ControlCmd, SCENE_CURSOR, TRACK_CURSOR};
pub use schedule::TripleBuffer;
pub use spsc::Spsc;

pub const MAX_TRACKS: usize = 32;
pub const MAX_SCENES: usize = 32;
pub const MAX_CLIPS_PER_TRACK: usize = MAX_SCENES; // columna = escena
pub const PPQ: u32 = 480; // mismos 480 PPQ que caja-core
pub const TICKS_PER_BAR: u64 = (PPQ as u64) * 4; // compás 4/4 (Fase 2: compases variables)
