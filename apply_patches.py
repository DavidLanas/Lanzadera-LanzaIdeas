import os

if not os.path.isdir("ui/src"):
    print("❌ Error: Ejecuta esto desde la raíz de Lanzadera-LanzaIdeas")
    exit(1)

patches_dir = "ui/src/patches"
os.makedirs(patches_dir, exist_ok=True)

files = {
    "mod.rs": """pub mod theme_tokens;
pub mod session_snapshot;
pub mod clip_grid;
pub mod build_snapshot;
""",
    "theme_tokens.rs": """use egui::Color32;
pub const GRID_LINE: Color32 = Color32::from_rgb(40, 40, 40);
pub const TRACK_HEADER_BG: Color32 = Color32::from_rgb(25, 25, 25);
pub const SCENE_COL_BG: Color32 = Color32::from_rgb(20, 20, 20);
pub const CLIP_BORDER: Color32 = Color32::from_rgb(60, 60, 60);
pub const METER_BG: Color32 = Color32::from_rgb(30, 30, 30);
pub const METER_FILL: Color32 = Color32::from_rgb(80, 200, 120);
pub const CLIP_BG_PLAYING: Color32 = Color32::from_rgb(55, 90, 70);
pub const CLIP_BG_TRIGGERED: Color32 = Color32::from_rgb(80, 80, 40);
pub const CLIP_BG_RECORDING: Color32 = Color32::from_rgb(120, 40, 40);
pub const CLIP_BG_STOPPED: Color32 = Color32::from_rgb(45, 45, 50);
pub const PROGRESS_FILL: Color32 = Color32::from_rgb(100, 180, 255);
pub const BADGE_TEXT: Color32 = Color32::from_rgb(160, 160, 160);
pub const TEXT_COLOR: Color32 = Color32::from_rgb(220, 220, 220);
pub const RED: Color32 = Color32::from_rgb(220, 60, 60);
pub const GRAY: Color32 = Color32::from_rgb(80, 80, 80);
pub const HEADER_H: f32 = 48.0;
pub const MIN_CELL_W: f32 = 100.0; pub const MAX_CELL_W: f32 = 200.0;
pub const MIN_CELL_H: f32 = 40.0; pub const MAX_CELL_H: f32 = 80.0;
pub const TRACK_HEADER_W: f32 = 160.0;
pub const FONT_SMALL: f32 = 10.0; pub const FONT_BASE: f32 = 11.0;
pub const FONT_MEDIUM: f32 = 12.0; pub const FONT_LARGE: f32 = 14.0;
""",
    "session_snapshot.rs": """use egui::Color32;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ClipStateView { #[default] Empty, Triggered, Playing, Stopping, Recording, Overdubbing, RecQueued }
#[derive(Clone, Debug)]
pub struct ClipSnapshot {
    pub name: String, pub color: Color32, pub state: ClipStateView,
    pub progress: f32, pub loop_bars: u8, pub quantize_bars: u8,
    pub peaks: Vec<f32>, pub follow_icon: Option<&'static str>,
}
impl Default for ClipSnapshot {
    fn default() -> Self { Self { name: String::new(), color: Color32::TRANSPARENT, state: ClipStateView::Empty, progress: 0.0, loop_bars: 0, quantize_bars: 0, peaks: Vec::new(), follow_icon: None } }
}
#[derive(Clone, Debug)]
pub struct TrackSnapshot {
    pub name: String, pub color: Color32, pub armed: bool, pub muted: bool,
    pub soloed: bool, pub meter_level: f32, pub clips: Vec<ClipSnapshot>,
}
#[derive(Clone, Debug, Default)]
pub struct TransportSnapshot {
    pub bar_beat: (u32, u32), pub beat_in_bar: u32, pub bpm: f64,
    pub quantize_bars: u8, pub rec_queued: bool, pub metronome: bool, pub playing: bool,
}
#[derive(Clone, Debug, Default)]
pub struct SessionSnapshot {
    pub tracks: Vec<TrackSnapshot>, pub scene_count: usize, pub transport: TransportSnapshot,
}
""",
    "clip_grid.rs": """use egui::{Align2, Button, Color32, FontId, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, Ui, Vec2};
use super::theme_tokens as theme; use super::session_snapshot::*;
#[derive(Default, Debug)]
pub struct ClipGridActions {
    pub toggle_clip: Option<(usize, usize)>, pub toggle_arm: Option<usize>,
    pub toggle_mute: Option<usize>, pub toggle_solo: Option<usize>,
    pub stop_track: Option<usize>, pub stop_all: bool, pub set_quantize: Option<u8>,
}
pub fn render_session_matrix(ui: &mut Ui, s: &SessionSnapshot, a: &mut ClipGridActions) {
    render_top_bar(ui, &s.transport, a); ui.separator();
    let n_tracks = s.tracks.len().max(1); let n_scenes = s.scene_count.max(1);
    let cell_w = ((ui.available_width() - theme::TRACK_HEADER_W - 40.0) / n_tracks as f32).clamp(theme::MIN_CELL_W, theme::MAX_CELL_W);
    let cell_h = ((ui.available_height() - theme::HEADER_H) / n_scenes as f32).clamp(theme::MIN_CELL_H, theme::MAX_CELL_H);
    ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
        ui.horizontal(|ui| {
            let (c, _) = ui.allocate_exact_size(Vec2::new(40.0, theme::HEADER_H), Sense::hover());
            ui.painter().rect_filled(c, 0.0, theme::TRACK_HEADER_BG);
            for (i, t) in s.tracks.iter().enumerate() {
                track_header(ui, t, i, cell_w, theme::HEADER_H, a);
                if i + 1 < s.tracks.len() { let r = ui.cursor(); ui.painter().vline(r.left(), r.top()..=r.top() + theme::HEADER_H, Stroke::new(1.0, theme::GRID_LINE)); }
            }
        });
        let y = ui.cursor().top(); ui.painter().hline(ui.min_rect().x_range(), y, Stroke::new(1.0, theme::GRID_LINE));
        for si in 0..s.scene_count {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::new(40.0, cell_h), Sense::hover());
                ui.painter().rect_filled(r, 0.0, theme::SCENE_COL_BG);
                ui.painter().text(r.center(), Align2::CENTER_CENTER, format!("{}", si + 1), FontId::monospace(theme::FONT_SMALL), theme::GRID_LINE);
                for (ti, t) in s.tracks.iter().enumerate() { clip_cell(ui, t, t.clips.get(si), ti, si, cell_w, cell_h, a); }
            });
            if (si + 1) % 4 == 0 && si + 1 < s.scene_count { let y = ui.cursor().top(); ui.painter().hline(ui.min_rect().x_range(), y, Stroke::new(1.0, theme::GRID_LINE)); }
        }
    });
}
fn render_top_bar(ui: &mut Ui, t: &TransportSnapshot, a: &mut ClipGridActions) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{}.{}", t.bar_beat.0, t.bar_beat.1)).size(theme::FONT_LARGE).strong());
        ui.label(if t.metronome { "🔔" } else { "🔕" }); ui.label(format!("BPM {:.1}", t.bpm)); ui.separator();
        ui.label(format!("Q {}", t.quantize_bars));
        if ui.small_button("Q4").clicked() { a.set_quantize = Some(4); }
        if ui.small_button("Q1").clicked() { a.set_quantize = Some(1); }
        if t.rec_queued { let c = 4_i32 - (t.beat_in_bar as i32 % 4); ui.label(RichText::new(format!("● REC {}", c)).color(theme::RED).strong()); }
        if ui.button("■ Stop All").clicked() { a.stop_all = true; }
    });
}
fn track_header(ui: &mut Ui, t: &TrackSnapshot, track_idx: usize, w: f32, h: f32, a: &mut ClipGridActions) {
    let (outer, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover()); let p = ui.painter();
    p.rect_filled(outer, 0.0, theme::TRACK_HEADER_BG);
    p.rect_filled(Rect::from_min_size(outer.min, Vec2::new(w, 5.0)), 0.0, t.color);
    p.text(Pos2::new(outer.left() + 6.0, outer.top() + 8.0), Align2::LEFT_TOP, &t.name, FontId::proportional(theme::FONT_MEDIUM), theme::TEXT_COLOR);
    let m = Rect::from_min_size(Pos2::new(outer.left() + 6.0, outer.bottom() - 6.0), Vec2::new(w - 12.0, 3.0));
    p.rect_filled(m, 0.0, theme::METER_BG);
    p.rect_filled(Rect::from_min_size(m.min, Vec2::new(m.width() * t.meter_level.clamp(0.0, 1.0), 3.0)), 0.0, theme::METER_FILL);
    let bw = 22.0; let bh = 16.0; let y = outer.top() + 26.0; let x0 = outer.left() + 6.0;
    let arm = Rect::from_min_size(Pos2::new(x0, y), Vec2::new(bw, bh));
    if ui.put(arm, Button::new(RichText::new("●").color(if t.armed { theme::RED } else { theme::GRAY })).small()).clicked() { a.toggle_arm = Some(track_idx); }
    let mute = arm.translate(Vec2::new(bw + 2.0, 0.0));
    if ui.put(mute, Button::new(RichText::new("M").color(if t.muted { theme::RED } else { theme::GRAY })).small()).clicked() { a.toggle_mute = Some(track_idx); }
    let solo = mute.translate(Vec2::new(bw + 2.0, 0.0));
    if ui.put(solo, Button::new(RichText::new("S").color(if t.soloed { theme::RED } else { theme::GRAY })).small()).clicked() { a.toggle_solo = Some(track_idx); }
    let stop = solo.translate(Vec2::new(bw + 2.0, 0.0));
    if ui.put(stop, Button::new("■").small()).clicked() { a.stop_track = Some(track_idx); }
}
fn clip_cell(ui: &mut Ui, t: &TrackSnapshot, c: Option<&ClipSnapshot>, ti: usize, si: usize, w: f32, h: f32, a: &mut ClipGridActions) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
    let st = c.map(|x| x.state).unwrap_or(ClipStateView::Empty);
    let bg = match st {
        ClipStateView::Empty => theme::SCENE_COL_BG, ClipStateView::Playing => theme::CLIP_BG_PLAYING,
        ClipStateView::Triggered | ClipStateView::RecQueued => theme::CLIP_BG_TRIGGERED,
        ClipStateView::Recording | ClipStateView::Overdubbing => theme::CLIP_BG_RECORDING,
        ClipStateView::Stopping => theme::CLIP_BG_STOPPED,
    };
    ui.painter().rect_filled(rect, 4.0, theme::CLIP_BORDER); ui.painter().rect_filled(rect.shrink(1.0), 4.0, bg);
    let btn = Rect::from_min_size(rect.min + Vec2::new(4.0, 4.0), Vec2::new(18.0, 18.0));
    let (sym, col) = match st {
        ClipStateView::Empty => ("■", theme::GRAY), ClipStateView::Playing => ("■", theme::TEXT_COLOR),
        ClipStateView::Stopping => ("▶", Color32::from_rgb(255, 160, 60)),
        ClipStateView::Recording | ClipStateView::Overdubbing => ("●", theme::RED),
        ClipStateView::RecQueued => { let time = ui.input(|i| i.time) as f32; ("●", if (time * 4.0).sin() > 0.0 { theme::RED } else { theme::GRAY }) },
        ClipStateView::Triggered => { let time = ui.input(|i| i.time) as f32; ("▶", if (time * 6.0).sin() > 0.0 { theme::RED } else { theme::TEXT_COLOR }) },
    };
    if ui.put(btn, Button::new(RichText::new(sym).color(col).size(theme::FONT_BASE))).clicked() { if c.is_some() { a.toggle_clip = Some((ti, si)); } }
    if let Some(x) = c {
        ui.painter().text(rect.min + Vec2::new(26.0, 5.0), Align2::LEFT_TOP, &x.name, FontId::proportional(theme::FONT_BASE), theme::TEXT_COLOR);
        if !x.peaks.is_empty() {
            let wv = Rect::from_min_max(Pos2::new(rect.left() + 26.0, rect.top() + 20.0), Pos2::new(rect.right() - 4.0, rect.bottom() - 12.0));
            draw_mini_waveform(ui.painter(), wv, &x.peaks);
        }
        let pr = Rect::from_min_size(Pos2::new(rect.left() + 4.0, rect.bottom() - 8.0), Vec2::new(rect.width() - 8.0, 4.0));
        ui.painter().rect_filled(pr, 2.0, theme::GRID_LINE);
        ui.painter().rect_filled(Rect::from_min_size(pr.min, Vec2::new(pr.width() * x.progress.clamp(0.0, 1.0), 4.0)), 2.0, theme::PROGRESS_FILL);
        ui.painter().text(rect.right_top() + Vec2::new(-4.0, 4.0), Align2::RIGHT_TOP, format!("{}b Q{}", x.loop_bars, x.quantize_bars), FontId::monospace(9.0), theme::BADGE_TEXT);
        if let Some(icon) = x.follow_icon { ui.painter().text(rect.right_bottom() + Vec2::new(-4.0, -14.0), Align2::RIGHT_BOTTOM, icon, FontId::monospace(10.0), theme::BADGE_TEXT); }
    } else {
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, "■", FontId::proportional(theme::FONT_BASE), theme::GRAY);
        if t.armed { ui.painter().text(rect.left_center() + Vec2::new(6.0, 0.0), Align2::LEFT_CENTER, "●", FontId::proportional(theme::FONT_SMALL), theme::RED); }
    }
    if resp.hovered() { ui.painter().rect_filled(rect.shrink(0.5), 4.0, Color32::from_rgba_unmultiplied(100, 180, 255, 30)); }
}
fn draw_mini_waveform(painter: &egui::Painter, rect: Rect, peaks: &[f32]) {
    if peaks.is_empty() || rect.width() < 2.0 { return; }
    let n = peaks.len(); let mid = rect.center().y; let half = rect.height() * 0.5; let stroke = Stroke::new(1.0, theme::PROGRESS_FILL);
    for (i, p) in peaks.iter().enumerate() {
        let x = rect.left() + (i as f32 / (n - 1).max(1) as f32) * rect.width();
        let amp = p.abs().clamp(0.0, 1.0) * half;
        painter.line_segment([Pos2::new(x, mid - amp), Pos2::new(x, mid + amp)], stroke);
    }
}
""",
    "build_snapshot.rs": """use egui::Color32;
// NOTA: Si tu crate se llama 'core' en lugar de 'lanzadera_core', ajusta estos imports.
use lanzadera_engine::State;
use lanzadera_core::track::Track;
use lanzadera_core::clip::{Clip, ClipState as RealClipState, FollowAction};
use super::session_snapshot::*;

pub fn build_snapshot(state: &State) -> SessionSnapshot {
    let tracks: Vec<TrackSnapshot> = state.tracks.iter().map(snapshot_track).collect();
    let scene_count = compute_scene_count(&tracks);
    let transport = snapshot_transport(state);
    SessionSnapshot { tracks, scene_count, transport }
}
fn snapshot_track(t: &Track) -> TrackSnapshot {
    TrackSnapshot {
        name: t.name.clone(), color: rgb(t.color), armed: t.arm, muted: t.mute, soloed: t.solo,
        meter_level: 0.0, // TODO: conectar con medidor real del engine
        clips: t.clips.iter().map(|opt| match opt { Some(c) => snapshot_clip(c), None => ClipSnapshot::default() }).collect(),
    }
}
fn snapshot_clip(c: &Clip) -> ClipSnapshot {
    ClipSnapshot {
        name: c.name.clone(), color: rgb(c.color), state: map_clip_state(&c.state),
        progress: 0.0, loop_bars: c.loop_bars, quantize_bars: quantize_to_bars(&c.quantize),
        peaks: generate_peaks(&c.midi_notes), follow_icon: follow_icon(&c.follow_action),
    }
}
fn rgb(c: [u8; 3]) -> Color32 { Color32::from_rgb(c[0], c[1], c[2]) }
fn map_clip_state(s: &RealClipState) -> ClipStateView {
    use RealClipState as S;
    match s {
        S::Empty => ClipStateView::Empty, S::Triggered => ClipStateView::Triggered,
        S::Playing => ClipStateView::Playing, S::Stopping => ClipStateView::Stopping,
        S::Recording => ClipStateView::Recording, S::Overdubbing => ClipStateView::Overdubbing,
        S::RecQueued => ClipStateView::RecQueued,
    }
}
fn follow_icon(fa: &FollowAction) -> Option<&'static str> {
    match fa {
        FollowAction::None => None, FollowAction::Next => Some("→"), FollowAction::Previous => Some("←"),
        FollowAction::First => Some("⤒"), FollowAction::Last => Some("⤓"), FollowAction::Stop => Some("■"),
        FollowAction::Random => Some("?"), FollowAction::Again => Some("↻"), FollowAction::JumpTo(_) => Some("↦"),
    }
}
fn quantize_to_bars(_q: &lanzadera_core::clip::Quantize) -> u8 { 4 } // TODO: mapear tu enum real
fn generate_peaks(_notes: &[lanzadera_core::clip::MidiNote]) -> Vec<f32> { Vec::new() } // TODO: derivar de midi_notes
fn compute_scene_count(tracks: &[TrackSnapshot]) -> usize {
    let max_used = tracks.iter().flat_map(|t| t.clips.iter().enumerate())
        .filter(|(_, c)| !matches!(c.state, ClipStateView::Empty)).map(|(i, _)| i + 1).max().unwrap_or(0);
    (((max_used + 3) / 4) * 4).max(8)
}
fn snapshot_transport(_state: &State) -> TransportSnapshot {
    // TODO: Rellena con los valores reales de tu State (bar, beat, bpm, etc.)
    TransportSnapshot::default()
}
"""
}

for filename, content in files.items():
    filepath = os.path.join(patches_dir, filename)
    with open(filepath, "w", encoding="utf-8") as f:
        f.write(content)
    print(f"✅ Creado: {filepath}")

print("\\n🎉 ¡Archivos de parche generados con éxito!")
