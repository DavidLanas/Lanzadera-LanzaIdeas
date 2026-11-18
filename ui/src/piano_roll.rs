//! PianoRoll (Fase 3): Draw/Select/Cut, mover y redimensionar notas, carril de
//! velocity, quantize configurable y playhead en tiempo real.
//! Atajos: D = dibujar · S = seleccionar · C = cuchillo · Supr = borrar selección.
use egui::{pos2, vec2, Align2, Color32, FontId, Rect, Rounding, Sense, Stroke};
use lanzadera_core::{MidiNote, TICKS_PER_BAR};
use lanzadera_engine::shared::SessionShared;
use crate::theme::T;

const QUANTS: [(u32, &str); 6] = [
    (60, "1/32"), (120, "1/16"), (240, "1/8"), (480, "1/4"), (960, "1/2"), (1920, "1/1"),
];
const KEY_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

fn is_black(k: u8) -> bool { matches!(k % 12, 1 | 3 | 6 | 8 | 10) }
fn key_name(k: u8) -> String { format!("{}{}", KEY_NAMES[(k % 12) as usize], (k as i32 / 12) - 1) }

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RollMode { Draw, Select, Cut }

#[derive(Clone)]
struct Drag { resize: bool, start_x: f32, orig: Vec<(usize, u32, u32)> }

pub struct PianoRoll {
    pub open: bool,
    pub target: Option<(usize, usize)>,
    pub mode: RollMode,
    pub px_per_tick: f32,
    pub row_h: f32,
    pub base_key: u8,
    pub n_rows: u8,
    pub quant: u32,
    pub sel: Vec<usize>,
    drag: Option<Drag>,
}

impl Default for PianoRoll {
    fn default() -> Self {
        PianoRoll {
            open: false, target: None, mode: RollMode::Draw,
            px_per_tick: 0.12, row_h: 14.0, base_key: 36, n_rows: 36,
            quant: 120, sel: Vec::new(), drag: None,
        }
    }
}

impl PianoRoll {
    pub fn open_for(&mut self, t: usize, s: usize) {
        self.open = true; self.target = Some((t, s)); self.sel.clear(); self.drag = None;
    }

    pub fn show(&mut self, ctx: &egui::Context, shared: &SessionShared) {
        if !self.open { return; }
        let Some((t, s)) = self.target else { return };
        let clip = {
            shared.state.lock().unwrap().tracks.get(t)
                .and_then(|tr| tr.clips.get(s)).cloned().flatten()
        };
        let Some(clip) = clip else { self.open = false; return };
        let mut notes = clip.midi_notes.clone();
        let loop_ticks = clip.loop_bars.max(1) as u32 * TICKS_PER_BAR as u32;
        let mut changed = false;
        let mut close = false;

        if ctx.input(|i| i.key_pressed(egui::Key::D)) { self.mode = RollMode::Draw; }
        if ctx.input(|i| i.key_pressed(egui::Key::S)) { self.mode = RollMode::Select; }
        if ctx.input(|i| i.key_pressed(egui::Key::C)) { self.mode = RollMode::Cut; }
        if ctx.input(|i| i.key_pressed(egui::Key::Delete)) && !self.sel.is_empty() {
            let mut idx = self.sel.clone();
            idx.sort_unstable(); idx.reverse();
            for i in idx { if i < notes.len() { notes.remove(i); } }
            self.sel.clear();
            changed = true;
        }

        egui::TopBottomPanel::bottom("piano_roll").resizable(true).default_height(300.0).show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(format!("🎹 {} · Pista {} · Escena {}", clip.name, t + 1, s + 1)).strong());
                ui.separator();
                for (m, lbl) in [(RollMode::Draw, "✏ D"), (RollMode::Select, "⬚ S"), (RollMode::Cut, "✂ C")] {
                    if ui.add(egui::Button::new(lbl).small()
                        .fill(if self.mode == m { T.accent.gamma_multiply(0.5) } else { T.panel })).clicked() {
                        self.mode = m;
                    }
                }
                ui.separator();
                ui.label("Grid");
                let qname = QUANTS.iter().find(|(q, _)| *q == self.quant).map(|(_, n)| *n).unwrap_or("1/16");
                egui::ComboBox::from_id_source("roll_quant").selected_text(qname).show_ui(ui, |ui| {
                    for (q, n) in QUANTS { ui.selectable_value(&mut self.quant, q, n); }
                });
                if ui.button("Cuantizar").clicked() {
                    let q = self.quant.max(1);
                    let targets: Vec<usize> = if self.sel.is_empty() { (0..notes.len()).collect() } else { self.sel.clone() };
                    for i in targets {
                        if let Some(n) = notes.get_mut(i) {
                            n.tick_start = ((n.tick_start as f32 / q as f32).round() as u32) * q;
                        }
                    }
                    changed = true;
                }
                ui.separator();
                ui.label("Zoom");
                ui.add(egui::Slider::new(&mut self.px_per_tick, 0.03..=0.6).show_value(false));
                if ui.button("▲").clicked() { self.base_key = (self.base_key + 12).min(96); }
                if ui.button("▼").clicked() { self.base_key = self.base_key.saturating_sub(12); }
                ui.label(format!("{} notas · {} compases", notes.len(), clip.loop_bars));
                if ui.button("✖").clicked() { close = true; }
            });
            ui.separator();

            let key_w = 38.0;
            let w = (loop_ticks as f32 * self.px_per_tick).max(300.0);
            let h = self.n_rows as f32 * self.row_h;
            // Teclado fijo a la izquierda + área de notas scrollable.
            // Antes el teclado se superponía al grid y ocultaba ~2.5 pasos
            // del primer compás (38px / 0.12 px/tick ≈ 317 ticks ≈ paso 3).
            ui.horizontal(|ui| {
                // Teclado fijo (no hace scroll).
                let (krect, _) = ui.allocate_exact_size(vec2(key_w, h + 52.0), Sense::hover());
                self.paint_keyboard(ui, krect.with_max_y(krect.top() + h));

                egui::ScrollArea::horizontal().id_source("roll_h").show(ui, |ui| {
                    ui.vertical(|ui| {
                        let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click_and_drag());
                        self.paint_notes(ui, rect, &notes, loop_ticks, t, s, shared);
                        changed |= self.interact_notes(&resp, rect, &mut notes, loop_ticks);
                        let (vrect, vresp) = ui.allocate_exact_size(vec2(w, 52.0), Sense::click_and_drag());
                        self.paint_velocity(ui, vrect, &notes);
                        changed |= self.interact_velocity(&vresp, vrect, &mut notes);
                    });
                });
            });
        });

        if close { self.open = false; }
        if changed {
            shared.edit(|st| {
                if let Some(tr) = st.tracks.get_mut(t) {
                    if let Some(Some(c)) = tr.clips.get_mut(s) { c.midi_notes = notes; }
                }
            });
            shared.sync_midi(t, s);
        }
    }

    fn key_at(&self, rect: Rect, y: f32) -> u8 {
        let row = ((y - rect.top()) / self.row_h) as i32;
        (self.base_key as i32 + (self.n_rows as i32 - 1 - row)).clamp(0, 127) as u8
    }
    fn row_y(&self, rect: Rect, key: u8) -> f32 {
        rect.top() + (self.n_rows as i32 - 1 - (key as i32 - self.base_key as i32)) as f32 * self.row_h
    }

    fn paint_notes(&self, ui: &egui::Ui, rect: Rect, notes: &[MidiNote], loop_ticks: u32,
                   t: usize, s: usize, shared: &SessionShared) {
        let p = ui.painter_at(rect);
        let px = self.px_per_tick;
        // Filas (teclas negras más oscuras).
        for r in 0..self.n_rows {
            let key = (self.base_key as i32 + self.n_rows as i32 - 1 - r as i32).clamp(0, 127) as u8;
            let y = rect.top() + r as f32 * self.row_h;
            let col = if is_black(key) { T.piano_black } else { T.piano_white.gamma_multiply(0.16) };
            p.rect_filled(Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), self.row_h)), Rounding::ZERO, col);
        }
        // Rejilla temporal.
        let q = self.quant.max(1);
        let mut tick = 0u32;
        while tick <= loop_ticks {
            let x = rect.left() + tick as f32 * px;
            let bar = tick % TICKS_PER_BAR as u32 == 0;
            p.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())],
                Stroke::new(1.0, Color32::from_gray(if bar { 90 } else { 48 })));
            tick += q;
        }
        // Notas.
        for (i, n) in notes.iter().enumerate() {
            let y = self.row_y(rect, n.note);
            if y + self.row_h < rect.top() || y > rect.bottom() { continue; }
            let x = rect.left() + n.tick_start as f32 * px;
            let w = (n.tick_dur as f32 * px).max(3.0);
            let r = Rect::from_min_size(pos2(x, y + 1.0), vec2(w, self.row_h - 2.0));
            let v = n.vel as f32 / 127.0;
            let col = Color32::from_rgb((70.0 + 150.0 * v) as u8, (120.0 + 60.0 * v) as u8, 235);
            p.rect_filled(r, Rounding::same(3.0), col);
            if self.sel.contains(&i) {
                p.rect_stroke(r, Rounding::same(3.0), Stroke::new(1.5, Color32::WHITE));
            }
        }
        // Playhead en tiempo real.
        if shared.rt.active_scene(t) == Some(s as u8) {
            let x = rect.left() + shared.rt.playhead(t) as f32 * px;
            p.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(2.0, T.piano_head));
        }
    }

    /// Teclado fijo a la izquierda (fuera del ScrollArea) para que el
    /// primer compás se vea completo desde el paso 1.
    fn paint_keyboard(&self, ui: &egui::Ui, rect: Rect) {
        let p = ui.painter_at(rect);
        p.rect_filled(rect, Rounding::ZERO, T.panel.gamma_multiply(0.96));
        for r in 0..self.n_rows {
            let key = (self.base_key as i32 + self.n_rows as i32 - 1 - r as i32).clamp(0, 127) as u8;
            let y = rect.top() + r as f32 * self.row_h;
            if is_black(key) {
                p.rect_filled(
                    Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), self.row_h)),
                    Rounding::ZERO,
                    Color32::from_gray(48),
                );
            }
            p.text(
                pos2(rect.left() + 4.0, y + self.row_h * 0.5),
                Align2::LEFT_CENTER,
                key_name(key),
                FontId::proportional(9.0),
                T.text_dim,
            );
        }
    }

    fn interact_notes(&mut self, resp: &egui::Response, rect: Rect, notes: &mut Vec<MidiNote>, loop_ticks: u32) -> bool {
        let mut changed = false;
        let Some(pos) = resp.interact_pointer_pos() else { return false };
        let px = self.px_per_tick;
        let q = self.quant.max(1);
        let tk = ((pos.x - rect.left()) / px).max(0.0);
        let key = self.key_at(rect, pos.y);
        let hit = notes.iter().position(|n| n.note == key
            && tk >= n.tick_start as f32 && tk < (n.tick_start + n.tick_dur) as f32);

        if resp.drag_started() || (resp.clicked() && self.drag.is_none()) {
            match self.mode {
                RollMode::Cut => {
                    if let Some(i) = hit { notes.remove(i); self.sel.clear(); changed = true; }
                }
                _ => {
                    if let Some(i) = hit {
                        if !self.sel.contains(&i) { self.sel = vec![i]; }
                        let n = notes[i];
                        let end_x = rect.left() + (n.tick_start + n.tick_dur) as f32 * px;
                        let resize = (pos.x - end_x).abs() < 6.0;
                        let idxs = if resize { vec![i] } else { self.sel.clone() };
                        self.drag = Some(Drag {
                            resize, start_x: pos.x,
                            orig: idxs.iter().filter_map(|&i| notes.get(i).map(|n| (i, n.tick_start, n.tick_dur))).collect(),
                        });
                    } else if self.mode == RollMode::Draw {
                        let start = ((tk / q as f32).floor() as u32) * q;
                        notes.push(MidiNote {
                            tick_start: start.min(loop_ticks.saturating_sub(1)),
                            tick_dur: q, note: key, vel: 100, channel: 0, occurrence: 0, probability: 1.0,
                        });
                        self.sel = vec![notes.len() - 1];
                        changed = true;
                    } else { self.sel.clear(); }
                }
            }
        }
        if resp.dragged() {
            if let Some(d) = self.drag.clone() {
                let dt = ((pos.x - d.start_x) / px / q as f32).round() as i32 * q as i32;
                for (i, st0, du0) in &d.orig {
                    if let Some(n) = notes.get_mut(*i) {
                        if d.resize { n.tick_dur = ((*du0 as i32 + dt).max(q as i32)) as u32; }
                        else { n.tick_start = (*st0 as i32 + dt).max(0) as u32; }
                    }
                }
                changed = true;
            }
        }
        if resp.drag_stopped() { self.drag = None; }
        if resp.secondary_clicked() {
            if let Some(i) = hit { notes.remove(i); self.sel.clear(); changed = true; }
        }
        changed
    }

    fn paint_velocity(&self, ui: &egui::Ui, rect: Rect, notes: &[MidiNote]) {
        let p = ui.painter_at(rect);
        p.rect_filled(rect, Rounding::same(3.0), T.strip_bg);
        let px = self.px_per_tick;
        for (i, n) in notes.iter().enumerate() {
            let x = rect.left() + n.tick_start as f32 * px;
            if x < rect.left() - 4.0 || x > rect.right() { continue; }
            let h = (n.vel as f32 / 127.0) * (rect.height() - 6.0);
            let col = if self.sel.contains(&i) { T.accent } else { T.meter_low };
            p.rect_filled(Rect::from_min_size(pos2(x, rect.bottom() - 3.0 - h), vec2(3.0, h)),
                          Rounding::same(1.0), col);
        }
        p.text(rect.left_top() + vec2(44.0, 2.0), Align2::LEFT_TOP, "Velocity",
               FontId::proportional(9.0), T.text_dim);
    }

    fn interact_velocity(&mut self, resp: &egui::Response, rect: Rect, notes: &mut Vec<MidiNote>) -> bool {
        if !(resp.dragged() || resp.clicked()) { return false; }
        let Some(pos) = resp.interact_pointer_pos() else { return false };
        let px = self.px_per_tick;
        let tk = ((pos.x - rect.left()) / px).max(0.0);
        let hit = notes.iter().position(|n| tk >= n.tick_start as f32
            && tk < (n.tick_start + n.tick_dur.max(1)) as f32);
        if let Some(i) = hit {
            let v = ((rect.bottom() - 3.0 - pos.y) / (rect.height() - 6.0)).clamp(0.0, 1.0);
            notes[i].vel = (v * 127.0).round() as u8;
            self.sel = vec![i];
            return true;
        }
        false
    }
}
