// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
use egui::Color32;
use lanzadera_core::session::SessionState;
use lanzadera_core::track::Track;
use lanzadera_core::clip::{Clip, ClipState as RealClipState, FollowAction, Quantize, MidiNote};
use super::session_snapshot::*;

pub fn build_snapshot(state: &SessionState) -> SessionSnapshot {
    let groove_idx = state.tracks.iter().position(|t| t.name.eq_ignore_ascii_case("Groove"));
    let tracks: Vec<TrackSnapshot> = state.tracks.iter().enumerate()
        .filter(|(i, _)| Some(*i) != groove_idx)
        .map(|(_, t)| snapshot_track(t))
        .collect();
    let scene_count = compute_scene_count_from_state(state);
    let transport = snapshot_transport(state);
    let groove = Some(GrooveSnapshotRow {
        name: "Groove".to_string(),
        clips: groove_idx
            .and_then(|i| state.tracks.get(i))
            .map(|t| (0..scene_count).map(|si| t.clips.get(si).and_then(|c| c.as_ref()).map(snapshot_clip)).collect())
            .unwrap_or_else(|| vec![None; scene_count]),
    });
    let scene_names = state.scenes.iter().map(|s| s.name.clone()).collect();
    SessionSnapshot {
        tracks,
        scene_count,
        transport,
        groove,
        selected_track: state.selected_track,
        selected_scene: state.selected_scene,
        scene_names,
    }
}

fn snapshot_track(t: &Track) -> TrackSnapshot {
    TrackSnapshot {
        name: t.name.clone(),
        color: Color32::from_rgb(t.color[0], t.color[1], t.color[2]),
        armed: t.arm,
        muted: t.mute,
        soloed: t.solo,
        meter_level: 0.0,
        clips: t.clips.iter().map(|opt| opt.as_ref().map(snapshot_clip)).collect(),
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
        peaks: crate::waveform::overview_from_notes(&c.midi_notes,
            c.loop_bars.max(1) as u32 * lanzadera_core::TICKS_PER_BAR as u32, 64),
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
        _ => 1,
    }
}

#[allow(dead_code)]
fn generate_peaks(notes: &[MidiNote]) -> Vec<f32> {
    if notes.is_empty() { return Vec::new(); }
    const BUCKETS: usize = 64;
    let end = notes.iter()
        .map(|n| n.tick_start + n.tick_dur.max(1))
        .max().unwrap_or(1).max(1) as u64;
    let mut out = vec![0.0f32; BUCKETS];
    for n in notes {
        let a = (n.tick_start as u64 * BUCKETS as u64 / end) as usize;
        let b = (((n.tick_start + n.tick_dur.max(1)) as u64 * BUCKETS as u64 / end) as usize)
            .max(a + 1).min(BUCKETS);
        let v = n.vel as f32 / 127.0;
        for k in a..b { if v > out[k] { out[k] = v; } }
    }
    out
}

fn compute_scene_count_from_state(state: &SessionState) -> usize {
    let max_used = state.tracks.iter()
        .flat_map(|t| t.clips.iter().enumerate())
        .filter(|(_, c)| c.is_some())
        .map(|(i, _)| i + 1)
        .max()
        .unwrap_or(0);
    (((max_used + 3) / 4) * 4).max(8)
}

fn snapshot_transport(state: &SessionState) -> TransportSnapshot {
    TransportSnapshot {
        bar_beat: (1, 1),
        beat_in_bar: 0,
        bpm: state.tempo,
        quantize_bars: quantize_to_bars(&state.quantize),
        rec_queued: state.recording,
        metronome: state.metronome,
        playing: state.playing,
    }
}
