import os, shutil, datetime

if not os.path.isdir("ui/src"):
    print("❌ Ejecuta esto desde la raíz de Lanzadera-LanzaIdeas")
    exit(1)

ts = datetime.datetime.now().strftime("%Y%m%d_%H%M%S")
def backup(p):
    if os.path.isfile(p):
        shutil.copy(p, p + f".bak_{ts}")

patches = "ui/src/patches"
os.makedirs(patches, exist_ok=True)

# ── 1) session_snapshot.rs: preservar Option (vacío vs clip detenido) ──
p = f"{patches}/session_snapshot.rs"
backup(p)
s = open(p, encoding="utf-8").read()
s = s.replace("pub clips: Vec<ClipSnapshot>,", "pub clips: Vec<Option<ClipSnapshot>>,")
open(p, "w", encoding="utf-8").write(s)

# ── 2) build_snapshot.rs: mapear Option y contar escenas con clip ──
p = f"{patches}/build_snapshot.rs"
backup(p)
open(p, "w", encoding="utf-8").write("""use egui::Color32;
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
        _ => 1,
    }
}

fn generate_peaks(_notes: &[MidiNote]) -> Vec<f32> {
    Vec::new()
}

fn compute_scene_count(tracks: &[TrackSnapshot]) -> usize {
    let max_used = tracks.iter()
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
""")

# ── 3) clip_grid.rs: acciones completas + menú contextual + foco ──
p = f"{patches}/clip_grid.rs"
backup(p)
open(p, "w", encoding="utf-8").write("""use egui::{vec2, Align2, Color32, FontId, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, Ui, Vec2};
use super::theme_tokens as theme;
use super::session_snapshot::*;

#[derive(Default, Debug)]
pub struct ClipGridActions {
    pub toggle_clip: Option<(usize, usize)>,
    pub launch_clip: Option<(usize, usize)>,
    pub select_clip: Option<(usize, usize)>,
    pub create_clip: Option<(usize, usize)>,
    pub delete_clip: Option<(usize, usize)>,
    pub duplicate_clip: Option<(usize, usize)>,
    pub open_piano_roll: Option<(usize, usize)>,
    pub open_audio_modal: Option<(usize, usize)>,
    pub load_file: Option<(usize, usize, String)>,
    pub select_track: Option<usize>,
    pub toggle_arm: Option<usize>,
    pub toggle_mute: Option<usize>,
    pub toggle_solo: Option<usize>,
    pub stop_track: Option<usize>,
    pub stop_all: bool,
    pub set_quantize: Option<u8>,
    pub launch_scene: Option<usize>,
}

pub fn render_session_matrix(ui: &mut Ui, s: &SessionSnapshot, a: &mut ClipGridActions) {
    render_top_bar(ui, &s.transport, a);
    ui.separator();

    let n_tracks = s.tracks.len().max(1);
    let n_scenes = s.scene_count.max(1);

    let avail = ui.available_size();
    let header_w = 140.0;
    let scene_header_h = 32.0;

    let cell_w = ((avail.x - header_w - 40.0) / n_scenes as f32).clamp(theme::MIN_CELL_W, theme::MAX_CELL_W);
    let cell_h = ((avail.y - scene_header_h - 40.0) / n_tracks as f32).clamp(theme::MIN_CELL_H, theme::MAX_CELL_H);

    ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
        egui::Grid::new("clip_launcher_grid")
            .spacing(vec2(4.0, 4.0))
            .show(ui, |ui| {
                ui.allocate_exact_size(vec2(header_w, scene_header_h), Sense::hover());

                for si in 0..n_scenes {
                    scene_header(ui, si, cell_w, scene_header_h, a);
                }
                ui.end_row();

                for (ti, track) in s.tracks.iter().enumerate() {
                    track_header(ui, track, ti, header_w, cell_h, a);
                    for si in 0..n_scenes {
                        let clip = track.clips.get(si).and_then(|c| c.as_ref());
                        clip_cell(ui, track, clip, ti, si, cell_w, cell_h, a);
                    }
                    ui.end_row();
                }
            });
    });
}

fn render_top_bar(ui: &mut Ui, t: &TransportSnapshot, a: &mut ClipGridActions) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{}.{}", t.bar_beat.0, t.bar_beat.1)).size(theme::FONT_LARGE).strong());
        ui.label(if t.metronome { "🔔" } else { "🔕" });
        ui.label(format!("BPM {:.1}", t.bpm));
        ui.separator();
        ui.label(format!("Q {}", t.quantize_bars));
        if ui.small_button("Q4").clicked() { a.set_quantize = Some(4); }
        if ui.small_button("Q1").clicked() { a.set_quantize = Some(1); }
        if t.rec_queued {
            let c = 4_i32 - (t.beat_in_bar as i32 % 4);
            ui.label(RichText::new(format!("● REC {}", c)).color(theme::RED).strong());
        }
        if ui.button("■ Stop All").clicked() { a.stop_all = true; }
    });
}

fn scene_header(ui: &mut Ui, si: usize, w: f32, h: f32, a: &mut ClipGridActions) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
    ui.painter().rect_filled(rect, 4.0, theme::SCENE_COL_BG);
    ui.painter().rect_stroke(rect, 4.0, Stroke::new(1.0, theme::GRID_LINE));

    ui.painter().text(
        rect.left_center() + Vec2::new(6.0, 0.0),
        Align2::LEFT_CENTER,
        format!("Escena {}", si + 1),
        FontId::proportional(theme::FONT_BASE),
        theme::TEXT_COLOR,
    );

    let btn_rect = Rect::from_min_size(rect.right_center() - Vec2::new(24.0, 10.0), Vec2::new(18.0, 18.0));
    ui.painter().rect_filled(btn_rect, 2.0, Color32::from_gray(60));
    ui.painter().text(btn_rect.center(), Align2::CENTER_CENTER, "▶", FontId::proportional(12.0), theme::TEXT_COLOR);

    if resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            if btn_rect.contains(pos) {
                a.launch_scene = Some(si);
            }
        }
    }

    if resp.hovered() {
        ui.painter().rect_filled(rect, 4.0, Color32::from_rgba_unmultiplied(100, 180, 255, 30));
    }
}

fn track_header(ui: &mut Ui, t: &TrackSnapshot, track_idx: usize, w: f32, h: f32, a: &mut ClipGridActions) {
    let (outer, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
    let p = ui.painter();

    p.rect_filled(outer, 0.0, theme::TRACK_HEADER_BG);
    p.rect_filled(Rect::from_min_size(outer.min, Vec2::new(w, 5.0)), 0.0, t.color);

    p.text(
        Pos2::new(outer.left() + 6.0, outer.top() + 8.0),
        Align2::LEFT_TOP,
        &t.name,
        FontId::proportional(theme::FONT_MEDIUM),
        theme::TEXT_COLOR,
    );

    let m = Rect::from_min_size(Pos2::new(outer.left() + 6.0, outer.bottom() - 8.0), Vec2::new(w - 12.0, 4.0));
    p.rect_filled(m, 0.0, theme::METER_BG);
    p.rect_filled(
        Rect::from_min_size(m.min, Vec2::new(m.width() * t.meter_level.clamp(0.0, 1.0), 4.0)),
        0.0, theme::METER_FILL,
    );

    let bw = 22.0; let bh = 16.0;
    let y = outer.top() + 26.0;
    let x0 = outer.left() + 6.0;

    let arm = Rect::from_min_size(Pos2::new(x0, y), Vec2::new(bw, bh));
    let mute = arm.translate(Vec2::new(bw + 2.0, 0.0));
    let solo = mute.translate(Vec2::new(bw + 2.0, 0.0));
    let stop = solo.translate(Vec2::new(bw + 2.0, 0.0));

    for (r, label, active) in [(arm, "●", t.armed), (mute, "M", t.muted), (solo, "S", t.soloed)] {
        let fill = if active { theme::RED } else { Color32::from_gray(50) };
        p.rect_filled(r, 2.0, fill);
        p.text(r.center(), Align2::CENTER_CENTER, label, FontId::proportional(10.0), theme::TEXT_COLOR);
    }
    p.rect_filled(stop, 2.0, Color32::from_gray(50));
    p.text(stop.center(), Align2::CENTER_CENTER, "■", FontId::proportional(10.0), theme::TEXT_COLOR);

    if resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            if arm.contains(pos) { a.toggle_arm = Some(track_idx); }
            else if mute.contains(pos) { a.toggle_mute = Some(track_idx); }
            else if solo.contains(pos) { a.toggle_solo = Some(track_idx); }
            else if stop.contains(pos) { a.stop_track = Some(track_idx); }
            else { a.select_track = Some(track_idx); }
        }
    }
}

fn clip_cell(ui: &mut Ui, t: &TrackSnapshot, c: Option<&ClipSnapshot>, ti: usize, si: usize, w: f32, h: f32, a: &mut ClipGridActions) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
    let has_clip = c.is_some();
    let st = c.map(|x| x.state).unwrap_or(ClipStateView::Empty);

    let bg = if !has_clip {
        theme::SCENE_COL_BG
    } else {
        match st {
            ClipStateView::Empty => theme::CLIP_BG_STOPPED,
            ClipStateView::Playing => theme::CLIP_BG_PLAYING,
            ClipStateView::Triggered | ClipStateView::RecQueued => theme::CLIP_BG_TRIGGERED,
            ClipStateView::Recording | ClipStateView::Overdubbing => theme::CLIP_BG_RECORDING,
            ClipStateView::Stopping => theme::CLIP_BG_STOPPED,
        }
    };

    ui.painter().rect_filled(rect, 4.0, theme::CLIP_BORDER);
    ui.painter().rect_filled(rect.shrink(1.0), 4.0, bg);

    let btn = Rect::from_min_size(rect.min + Vec2::new(4.0, 4.0), Vec2::new(18.0, 18.0));
    let (sym, col) = if !has_clip {
        (if t.armed { "＋" } else { "·" }, theme::GRAY)
    } else {
        match st {
            ClipStateView::Empty => ("▶", theme::TEXT_COLOR),
            ClipStateView::Playing => ("■", theme::TEXT_COLOR),
            ClipStateView::Stopping => ("▶", Color32::from_rgb(255, 160, 60)),
            ClipStateView::Recording | ClipStateView::Overdubbing => ("●", theme::RED),
            ClipStateView::RecQueued => {
                let time = ui.input(|i| i.time) as f32;
                ("●", if (time * 4.0).sin() > 0.0 { theme::RED } else { theme::GRAY })
            }
            ClipStateView::Triggered => {
                let time = ui.input(|i| i.time) as f32;
                ("▶", if (time * 6.0).sin() > 0.0 { theme::RED } else { theme::TEXT_COLOR })
            }
        }
    };

    ui.painter().rect_filled(btn, 2.0, Color32::from_gray(40));
    ui.painter().text(btn.center(), Align2::CENTER_CENTER, sym, FontId::proportional(12.0), col);

    if let Some(x) = c {
        ui.painter().text(
            rect.min + Vec2::new(26.0, 5.0),
            Align2::LEFT_TOP,
            &x.name,
            FontId::proportional(theme::FONT_BASE),
            theme::TEXT_COLOR,
        );

        if !x.peaks.is_empty() {
            let wv = Rect::from_min_max(
                Pos2::new(rect.left() + 26.0, rect.top() + 20.0),
                Pos2::new(rect.right() - 4.0, rect.bottom() - 14.0),
            );
            draw_mini_waveform(ui.painter(), wv, &x.peaks);
        }

        let pr = Rect::from_min_size(Pos2::new(rect.left() + 4.0, rect.bottom() - 10.0), Vec2::new(rect.width() - 8.0, 4.0));
        ui.painter().rect_filled(pr, 2.0, theme::GRID_LINE);
        ui.painter().rect_filled(
            Rect::from_min_size(pr.min, Vec2::new(pr.width() * x.progress.clamp(0.0, 1.0), 4.0)),
            2.0, theme::PROGRESS_FILL,
        );

        ui.painter().text(
            rect.right_top() + Vec2::new(-4.0, 4.0),
            Align2::RIGHT_TOP,
            format!("{}b Q{}", x.loop_bars, x.quantize_bars),
            FontId::monospace(9.0),
            theme::BADGE_TEXT,
        );

        if let Some(icon) = x.follow_icon {
            ui.painter().text(
                rect.right_bottom() + Vec2::new(-4.0, -14.0),
                Align2::RIGHT_BOTTOM,
                icon,
                FontId::monospace(10.0),
                theme::BADGE_TEXT,
            );
        }
    } else if t.armed {
        ui.painter().text(
            rect.left_center() + Vec2::new(6.0, 0.0),
            Align2::LEFT_CENTER,
            "●",
            FontId::proportional(theme::FONT_SMALL),
            theme::RED,
        );
    }

    if resp.hovered() {
        ui.painter().rect_filled(rect.shrink(0.5), 4.0, Color32::from_rgba_unmultiplied(100, 180, 255, 30));
    }

    // ── Interacciones (lógica original reconectada) ──
    if resp.double_clicked() {
        a.open_piano_roll = Some((ti, si));
    } else if resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            if btn.contains(pos) {
                if has_clip { a.toggle_clip = Some((ti, si)); }
                else if t.armed { a.create_clip = Some((ti, si)); }
            } else {
                a.select_clip = Some((ti, si));
                if has_clip { a.launch_clip = Some((ti, si)); }
                else if t.armed { a.create_clip = Some((ti, si)); }
            }
        }
    }

    // Menú contextual (click derecho)
    resp.context_menu(|ui| {
        if ui.button("🎹 Piano Roll").clicked() { a.open_piano_roll = Some((ti, si)); ui.close_menu(); }
        if ui.button("📂 Cargar audio/MIDI…").clicked() { a.open_audio_modal = Some((ti, si)); ui.close_menu(); }
        if has_clip {
            if ui.button("🗑 Borrar clip").clicked() { a.delete_clip = Some((ti, si)); ui.close_menu(); }
            if ui.button("⧉ Duplicar").clicked() { a.duplicate_clip = Some((ti, si)); ui.close_menu(); }
        }
        if ui.button("◎ Seleccionar").clicked() { a.select_clip = Some((ti, si)); ui.close_menu(); }
    });

    // Drop de archivos externos sobre la celda
    let dropped = ui.input(|i| i.raw.dropped_files.clone());
    if resp.hovered() && !dropped.is_empty() {
        if let Some(f) = dropped.first() {
            if let Some(path) = &f.path {
                a.load_file = Some((ti, si, path.to_string_lossy().into_owned()));
            }
        }
    }
}

fn draw_mini_waveform(painter: &egui::Painter, rect: Rect, peaks: &[f32]) {
    if peaks.is_empty() || rect.width() < 2.0 { return; }
    let n = peaks.len();
    let mid = rect.center().y;
    let half = rect.height() * 0.5;
    let stroke = Stroke::new(1.0, theme::PROGRESS_FILL);
    for (i, p) in peaks.iter().enumerate() {
        let x = rect.left() + (i as f32 / (n - 1).max(1) as f32) * rect.width();
        let amp = p.abs().clamp(0.0, 1.0) * half;
        painter.line_segment([Pos2::new(x, mid - amp), Pos2::new(x, mid + amp)], stroke);
    }
}
""")

# ── 4) view.rs: reconectar todas las acciones en el método clip_grid ──
p = "ui/src/view.rs"
backup(p)
src = open(p, encoding="utf-8").read()

sig = "fn clip_grid(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, shared: &SessionShared, audio_on: bool)"
idx = src.find(sig)
if idx == -1:
    print("❌ No encontré fn clip_grid en view.rs")
    exit(1)

start = src.find("{", idx)
depth = 0
i = start
end = None
while i < len(src):
    if src[i] == "{":
        depth += 1
    elif src[i] == "}":
        depth -= 1
        if depth == 0:
            end = i + 1
            break
    i += 1
if end is None:
    print("❌ No pude determinar el fin de clip_grid")
    exit(1)

new_method = """fn clip_grid(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context, shared: &SessionShared, _audio_on: bool) {
        let snapshot = {
            let st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            crate::patches::build_snapshot::build_snapshot(&st)
        };

        let mut actions = crate::patches::clip_grid::ClipGridActions::default();
        crate::patches::clip_grid::render_session_matrix(ui, &snapshot, &mut actions);

        use lanzadera_core::control::ControlCmd;
        use lanzadera_core::clip::Quantize;

        if let Some(t) = actions.select_track {
            shared.edit(|st| { st.selected_track = t; });
            self.inspector_focus = InspectorFocus::Track;
        }
        if let Some((t, s)) = actions.select_clip {
            shared.apply_control(ControlCmd::SelectClip { track: t as u8, scene: s as u8 });
            self.inspector_focus = InspectorFocus::Clip;
        }
        if let Some((t, s)) = actions.create_clip {
            shared.apply_control(ControlCmd::CreateClip { track: t as u8, scene: s as u8, bars: 8 });
            self.inspector_focus = InspectorFocus::Clip;
        }
        if let Some((t, s)) = actions.launch_clip {
            shared.apply_control(ControlCmd::LaunchClip { track: t as u8, scene: s as u8 });
        }
        if let Some((t, s)) = actions.toggle_clip {
            shared.apply_control(ControlCmd::LaunchClip { track: t as u8, scene: s as u8 });
        }
        if let Some(si) = actions.launch_scene {
            shared.apply_control(ControlCmd::LaunchScene { scene: si as u8 });
        }
        if let Some(t) = actions.toggle_arm {
            shared.apply_control(ControlCmd::ToggleArm { track: t as u8 });
        }
        if let Some(t) = actions.toggle_mute {
            shared.apply_control(ControlCmd::ToggleMute { track: t as u8 });
        }
        if let Some(t) = actions.toggle_solo {
            shared.apply_control(ControlCmd::ToggleSolo { track: t as u8 });
        }
        if let Some(t) = actions.stop_track {
            shared.apply_control(ControlCmd::StopClip { track: t as u8 });
        }
        if actions.stop_all {
            shared.apply_control(ControlCmd::StopAll);
        }
        if let Some(q) = actions.set_quantize {
            let q_enum = match q {
                1 => Quantize::One,
                2 => Quantize::Two,
                4 => Quantize::Four,
                _ => Quantize::One,
            };
            shared.edit(|st| st.quantize = q_enum);
        }
        if let Some((t, s)) = actions.delete_clip {
            shared.apply_control(ControlCmd::StopClip { track: t as u8 });
            shared.edit(|st| {
                if let Some(tr) = st.tracks.get_mut(t) {
                    if let Some(slot) = tr.clips.get_mut(s) { *slot = None; }
                }
                if st.selected_clip == Some((t, s)) { st.selected_clip = None; }
            });
            shared.sync_midi(t, s);
        }
        if let Some((t, s)) = actions.duplicate_clip {
            let target = shared.edit(|st| {
                if let Some(tr) = st.tracks.get_mut(t) {
                    let copy = tr.clips.get(s).and_then(|c| c.clone());
                    if let Some(c) = copy {
                        if let Some(empty) = tr.clips.iter_mut().position(|x| x.is_none()) {
                            tr.clips[empty] = Some(c);
                            return Some(empty);
                        }
                    }
                }
                None
            });
            if let Some(s2) = target { shared.sync_midi(t, s2); }
        }
        if let Some((t, s)) = actions.open_piano_roll {
            shared.apply_control(ControlCmd::SelectClip { track: t as u8, scene: s as u8 });
            self.inspector_focus = InspectorFocus::Clip;
        }
        if let Some((t, s)) = actions.open_audio_modal {
            self.audio_modal = Some((t, s));
            self.audio_path_input.clear();
        }
        if let Some((t, s, path)) = actions.load_file {
            shared.request_load(t, s, path);
        }
    }"""

src = src[:idx] + new_method + src[end:]
open(p, "w", encoding="utf-8").write(src)

print("✅ Lógica de clip launch reconectada: selección, inspector, lanzamiento, creación, borrado, duplicado, menú contextual y carga.")
