import os

if not os.path.isdir("ui/src"):
    print("❌ Ejecuta esto desde la raíz de Lanzadera-LanzaIdeas")
    exit(1)

patches_dir = "ui/src/patches"
os.makedirs(patches_dir, exist_ok=True)

# 1. build_snapshot.rs con los tipos REALES del repositorio
with open(f"{patches_dir}/build_snapshot.rs", "w", encoding="utf-8") as f:
    f.write("""use egui::Color32;
use lanzadera_core::session::SessionState;
use lanzadera_core::track::Track;
use lanzadera_core::clip::{Clip, ClipState as RealClipState, FollowAction, Quantize, MidiNote};
use super::session_snapshot::*;

pub fn build_snapshot(state: &SessionState) -> SessionSnapshot {
    let tracks: Vec<TrackSnapshot> = state.tracks.iter().map(snapshot_track).collect();
    let scene_count = compute_scene_count(&tracks);
    let transport = snapshot_transport(state);
    SessionSnapshot { tracks, scene_count, transport }
}

fn snapshot_track(t: &Track) -> TrackSnapshot {
    TrackSnapshot {
        name: t.name.clone(),
        color: Color32::from_rgb(t.color[0], t.color[1], t.color[2]),
        armed: t.arm,
        muted: t.mute,
        soloed: t.solo,
        meter_level: 0.0, // TODO: conectar con medidor real del engine
        clips: t.clips.iter().map(|opt| match opt {
            Some(c) => snapshot_clip(c),
            None => ClipSnapshot::default(),
        }).collect(),
    }
}

fn snapshot_clip(c: &Clip) -> ClipSnapshot {
    ClipSnapshot {
        name: c.name.clone(),
        color: Color32::from_rgb(c.color[0], c.color[1], c.color[2]),
        state: map_clip_state(&c.state),
        progress: 0.0,
        loop_bars: c.loop_bars,
        quantize_bars: quantize_to_bars(&c.quantize),
        peaks: generate_peaks(&c.midi_notes),
        follow_icon: follow_icon(&c.follow_action),
    }
}

fn map_clip_state(s: &RealClipState) -> ClipStateView {
    use RealClipState as S;
    match s {
        S::Empty => ClipStateView::Empty,
        S::Triggered => ClipStateView::Triggered,
        S::Playing => ClipStateView::Playing,
        S::Stopping => ClipStateView::Stopping,
        S::Recording => ClipStateView::Recording,
        S::Overdubbing => ClipStateView::Overdubbing,
        S::RecQueued => ClipStateView::RecQueued,
    }
}

fn follow_icon(fa: &FollowAction) -> Option<&'static str> {
    match fa {
        FollowAction::None => None,
        FollowAction::Next => Some("→"),
        FollowAction::Previous => Some("←"),
        FollowAction::First => Some("⤒"),
        FollowAction::Last => Some("⤓"),
        FollowAction::Stop => Some("■"),
        FollowAction::Random => Some("?"),
        FollowAction::Again => Some("↻"),
        FollowAction::JumpTo(_) => Some("↦"),
    }
}

fn quantize_to_bars(q: &Quantize) -> u8 {
    match q {
        Quantize::One => 1,
        Quantize::Two => 2,
        Quantize::Four => 4,
        Quantize::Eight => 8,
        _ => 1, // Immediate, ThirtySecond, etc. se tratan como 1 compás por defecto
    }
}

fn generate_peaks(_notes: &[MidiNote]) -> Vec<f32> {
    // TODO: derivar de midi_notes o audio_path
    Vec::new()
}

fn compute_scene_count(tracks: &[TrackSnapshot]) -> usize {
    let max_used = tracks.iter()
        .flat_map(|t| t.clips.iter().enumerate())
        .filter(|(_, c)| !matches!(c.state, ClipStateView::Empty))
        .map(|(i, _)| i + 1)
        .max()
        .unwrap_or(0);
    (((max_used + 3) / 4) * 4).max(8)
}

fn snapshot_transport(state: &SessionState) -> TransportSnapshot {
    TransportSnapshot {
        bar_beat: (1, 1), // TODO: calcular desde state.playing y tempo
        beat_in_bar: 0,
        bpm: state.tempo,
        quantize_bars: quantize_to_bars(&state.quantize),
        rec_queued: state.recording,
        metronome: state.metronome,
        playing: state.playing,
    }
}
""")

print("✅ Tipos corregidos en build_snapshot.rs (SessionState, SessionShared)")
