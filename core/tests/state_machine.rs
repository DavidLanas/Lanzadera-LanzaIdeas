//! Tests de la máquina de estados del launcher (Fase 1).
use lanzadera_core::*;

fn clip(q: Quantize) -> Clip { Clip { quantize: q, ..Default::default() } }

#[test]
fn sesion_por_defecto_8x8() {
    let s = SessionState::default();
    assert_eq!(s.tracks.len(), 8);
    assert_eq!(s.scenes.len(), 8);
    assert_eq!(s.tempo, 120.0);
    assert_eq!(s.tracks[0].clips.len(), MAX_SCENES);
}

#[test]
fn launch_cuantizado_queda_triggered() {
    let mut s = SessionState::default();
    s.set_clip(0, 0, clip(Quantize::One));
    s.launch_clip(0, 0);
    assert_eq!(s.tracks[0].clips[0].as_ref().unwrap().state, ClipState::Triggered);
    assert!(s.playing);
}

#[test]
fn launch_inmediato_queda_playing() {
    let mut s = SessionState::default();
    s.set_clip(0, 0, clip(Quantize::Immediate));
    s.launch_clip(0, 0);
    assert_eq!(s.tracks[0].clips[0].as_ref().unwrap().state, ClipState::Playing);
}

#[test]
fn stop_cuantizado_pasa_por_stopping() {
    let mut s = SessionState::default();
    s.set_clip(0, 0, clip(Quantize::Immediate));
    s.launch_clip(0, 0);
    s.quantize = Quantize::One;
    s.stop_track(0);
    assert_eq!(s.tracks[0].clips[0].as_ref().unwrap().state, ClipState::Stopping);
    s.quantize = Quantize::Immediate;
    s.stop_track(0);
    assert_eq!(s.tracks[0].clips[0].as_ref().unwrap().state, ClipState::Empty);
}

#[test]
fn launch_scene_solo_toca_clips_existentes() {
    let mut s = SessionState::default();
    s.set_clip(0, 2, clip(Quantize::One));
    s.set_clip(3, 2, clip(Quantize::One));
    s.launch_scene(2);
    assert_eq!(s.tracks[0].clips[2].as_ref().unwrap().state, ClipState::Triggered);
    assert_eq!(s.tracks[3].clips[2].as_ref().unwrap().state, ClipState::Triggered);
    assert!(s.tracks[1].clips[2].is_none());
}

#[test]
fn toggle_relanzar_detiene() {
    let mut s = SessionState::default();
    s.quantize = Quantize::Immediate;
    s.set_clip(0, 0, Clip { launch_mode: LaunchMode::Toggle, quantize: Quantize::Immediate, ..Default::default() });
    s.launch_clip(0, 0);
    assert_eq!(s.tracks[0].clips[0].as_ref().unwrap().state, ClipState::Playing);
    s.launch_clip(0, 0);
    assert_eq!(s.tracks[0].clips[0].as_ref().unwrap().state, ClipState::Empty);
}

#[test]
fn cursor_se_resuelve_en_comandos() {
    let mut s = SessionState::default();
    s.selected_track = 5;
    s.apply_control(ControlCmd::SetVolume { track: TRACK_CURSOR, value: 0.25 });
    assert!((s.tracks[5].volume - 0.25).abs() < 1e-6);
    s.apply_control(ControlCmd::ToggleMute { track: TRACK_CURSOR });
    assert!(s.tracks[5].mute);
}

#[test]
fn quantize_ticks_tabla() {
    assert_eq!(Quantize::Immediate.ticks(), 0);
    assert_eq!(Quantize::Eighth.ticks(), 240);
    assert_eq!(Quantize::Quarter.ticks(), 480);
    assert_eq!(Quantize::Half.ticks(), 960);
    assert_eq!(Quantize::One.ticks(), 1920);
    assert_eq!(Quantize::Two.ticks(), 3840);
    assert_eq!(Quantize::Four.ticks(), 7680);
    assert_eq!(Quantize::Eight.ticks(), 15360);
}

#[test]
fn json_roundtrip_conserva_el_clip() {
    let mut s = SessionState::default();
    s.set_clip(1, 3, Clip {
        name: "Verso".into(), color: [10, 200, 30], probability: 0.75,
        loop_bars: 4, loop_count: 2, follow_action: FollowAction::Next,
        midi_notes: vec![MidiNote { tick_start: 0, tick_dur: 240, note: 36, vel: 100, channel: 0, occurrence: 0, probability: 1.0 }],
        ..Default::default()
    });
    let back = SessionState::from_json(&s.to_json()).unwrap();
    let c = back.tracks[1].clips[3].as_ref().unwrap();
    assert_eq!(c.name, "Verso");
    assert_eq!(c.color, [10, 200, 30]);
    assert_eq!(c.loop_bars, 4);
    assert_eq!(c.follow_action, FollowAction::Next);
    assert_eq!(c.midi_notes.len(), 1);
    assert_eq!(back.tempo, 120.0);
}

#[test]
fn snapshot_pod_deriva_del_estado() {
    let mut s = SessionState::default();
    s.set_clip(2, 1, clip(Quantize::Immediate));
    s.launch_clip(2, 1);
    s.tracks[4].solo = true;
    let a = SessionAudio::from_state(&s);
    assert_eq!(a.track_count, 8);
    assert!(a.any_solo);
    assert!(a.tracks[2].active_clip.is_some());
    assert!(a.clip_meta(2, 1).exists);
    assert!(!a.clip_meta(0, 0).exists);
}
