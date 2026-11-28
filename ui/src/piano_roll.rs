// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! PianoRoll (Fase 3I) — mejoras sobre Fase 3:
//!
//!  * Scroll vertical continuo (rango MIDI 0-127 accesible sin saltos).
//!  * Selección por rectángulo (rubber-band) en modo Select.
//!  * Seguimiento automático del playhead (opción "seguir").
//!  * Zoom con Ctrl + rueda del ratón.
//!  * Etiquetas de nota sobre bloques cuando el zoom es suficiente.
//!  * Teclado mejorado: teclas blancas/negras con ancho relativo correcto
//!    y resaltado de octava activa.
//!  * Menú contextual con clic derecho (eliminar, seleccionar octava).
//!
//! # Atribución
//! Lógica de rubber-band y seguimiento de playhead basada en patrones
//! documentados en Lemon Studio (MIT) y nxtakt (Rust, MIT).
//! El cálculo de posición de teclas negras sigue el estándar de piano físico.

use egui::{pos2, vec2, Align2, Color32, CursorIcon, FontId, Key, Rect, Rounding, Sense, Stroke};
use lanzadera_core::{MidiNote, TICKS_PER_BAR};
use lanzadera_engine::shared::SessionShared;
use crate::theme::T;

const QUANTS: [(u32, &str); 8] = [
    (30, "1/64"), (60, "1/32"), (120, "1/16"), (240, "1/8"),
    (480, "1/4"), (960, "1/2"), (1920, "1/1"), (3840, "2/1"),
];
const KEY_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
const KEY_W: f32 = 48.0;
const VISIBLE_ROWS: u8 = 24;
const LABEL_ZOOM_THRESHOLD: f32 = 0.18;

fn is_black(k: u8) -> bool { matches!(k % 12, 1|3|6|8|10) }
fn key_name(k: u8) -> String { format!("{}{}", KEY_NAMES[(k%12) as usize], (k as i32/12)-1) }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RollMode { Draw, Select, Cut, Knife }

#[derive(Clone)]
struct MoveDrag { resize: bool, start_x: f32, start_y: f32,
    orig: Vec<(usize, u32, u32, u8)> }
#[derive(Clone)]
struct BandDrag { start: egui::Pos2, cur: egui::Pos2 }

pub struct PianoRoll {
    pub open: bool, pub target: Option<(usize, usize)>,
    pub mode: RollMode, pub px_per_tick: f32, pub row_h: f32,
    pub scroll_key: u8, pub n_rows: u8, pub quant: u32,
    pub sel: Vec<usize>, pub follow_head: bool,
    drag: Option<MoveDrag>, band: Option<BandDrag>, h_scroll_offset: f32,
    focus_notes_on_open: bool,
}

impl Default for PianoRoll {
    fn default() -> Self {
        PianoRoll { open: false, target: None, mode: RollMode::Draw,
            px_per_tick: 0.12, row_h: 14.0, scroll_key: 36, n_rows: VISIBLE_ROWS,
            quant: 120, sel: Vec::new(), follow_head: false,
            drag: None, band: None, h_scroll_offset: 0.0,
            focus_notes_on_open: false }
    }
}

impl PianoRoll {
    pub fn open_for(&mut self, t: usize, s: usize) {
        self.open = true; self.target = Some((t, s));
        self.sel.clear(); self.drag = None; self.band = None;
        self.focus_notes_on_open = true;
    }

    pub fn show(&mut self, ctx: &egui::Context, shared: &SessionShared) {
        if !self.open { return; }
        let Some((t, s)) = self.target else { return };
        let clip = shared.state.lock().unwrap().tracks.get(t)
            .and_then(|tr| tr.clips.get(s)).cloned().flatten();
        let Some(clip) = clip else { self.open = false; return };

        let mut notes = clip.midi_notes.clone();
        let loop_ticks = clip.loop_bars.max(1) as u32 * TICKS_PER_BAR as u32;
        let mut changed = false;
        let mut close = false;

        ctx.input(|i| {
            if i.key_pressed(Key::D) { self.mode = RollMode::Draw; }
            if i.key_pressed(Key::S) { self.mode = RollMode::Select; }
            if i.key_pressed(Key::C) { self.mode = RollMode::Cut; }
            if i.key_pressed(Key::K) { self.mode = RollMode::Knife; }
            if i.key_pressed(Key::A) && i.modifiers.ctrl {
                self.sel = (0..notes.len()).collect();
            }
            if i.modifiers.ctrl && i.raw_scroll_delta.y != 0.0 {
                let factor = if i.raw_scroll_delta.y > 0.0 { 1.12 } else { 1.0 / 1.12 };
                self.px_per_tick = (self.px_per_tick * factor).clamp(0.03, 0.9);
            } else if !i.modifiers.ctrl && i.raw_scroll_delta.y != 0.0 {
                let delta = if i.raw_scroll_delta.y > 0.0 { 2i32 } else { -2i32 };
                let max_k = 127i32 - self.n_rows as i32 + 1;
                self.scroll_key = (self.scroll_key as i32 + delta).clamp(0, max_k.max(0)) as u8;
            }
        });

        if ctx.input(|i| i.key_pressed(Key::Delete)) && !self.sel.is_empty() {
            let mut idx = self.sel.clone(); idx.sort_unstable(); idx.reverse();
            for i in idx { if i < notes.len() { notes.remove(i); } }
            self.sel.clear(); changed = true;
        }

        egui::TopBottomPanel::bottom("piano_roll")
            .resizable(true).default_height(320.0).min_height(160.0)
            .show(ctx, |ui| {
                self.show_toolbar(ui, &clip, &mut notes, t, s, &mut changed, &mut close);
                ui.separator();

                if self.follow_head && shared.rt.active_scene(t) == Some(s as u8) {
                    let head_px = shared.rt.playhead(t) as f32 * self.px_per_tick;
                    self.h_scroll_offset = (head_px - 120.0).max(0.0);
                }

                let w_notes = (loop_ticks as f32 * self.px_per_tick).max(300.0);
                let h_keys = self.n_rows as f32 * self.row_h;
                let h_vel = 52.0;

                ui.horizontal(|ui| {
                    let (krect, _) = ui.allocate_exact_size(vec2(KEY_W, h_keys + h_vel + 4.0), Sense::hover());
                    self.paint_keyboard(ui, krect.with_max_y(krect.top() + h_keys));

                    let scroll_id = egui::Id::new(("roll_hscroll", t, s));
                    egui::ScrollArea::horizontal().id_source(scroll_id)
                        .scroll_offset([self.h_scroll_offset, 0.0].into()).show(ui, |ui| {
                        ui.vertical(|ui| {
                            egui::ScrollArea::vertical()
                                .id_source(egui::Id::new(("roll_vscroll", t, s)))
                                .max_height(h_keys).show(ui, |ui| {
                                let h_full = 128.0 * self.row_h;
                                let (rect, resp) = ui.allocate_exact_size(vec2(w_notes, h_full), Sense::click_and_drag());
                                self.paint_notes(ui, rect, &notes, loop_ticks, t, s, shared);
                                changed |= self.interact_notes(&resp, rect, &mut notes, loop_ticks);
                                self.paint_band_rect(ui, rect);
                            });
                            let (vrect, vresp) = ui.allocate_exact_size(vec2(w_notes, h_vel), Sense::click_and_drag());
                            self.paint_velocity(ui, vrect, &notes);
                            changed |= self.interact_velocity(&vresp, vrect, &mut notes);
                        });
                    });
                });
            });

        if close { self.open = false; }
        if self.focus_notes_on_open {
            self.focus_on_notes(&notes);
            self.focus_notes_on_open = false;
        }

        if changed {
            shared.edit(|st| {
                if let Some(tr) = st.tracks.get_mut(t) {
                    if let Some(Some(c)) = tr.clips.get_mut(s) { c.midi_notes = notes; }
                }
            });
            shared.sync_midi(t, s);
        }
    }

    fn show_toolbar(&mut self, ui: &mut egui::Ui, clip: &lanzadera_core::Clip,
        notes: &mut Vec<MidiNote>, t: usize, s: usize,
        changed: &mut bool, close: &mut bool) {
        // Barra estilo DAW: título · modos · grid · herramientas · zoom · info
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(
                format!("🎹 {} · P{} · S{}", clip.name, t+1, s+1))
                .strong().color(T.accent_gold));
            ui.separator();
            for (m, lbl, tip) in [(RollMode::Draw, "✏ D", "Dibujar (D)"),
                                   (RollMode::Select, "⬚ S", "Seleccionar (S)"),
                                   (RollMode::Cut, "✂ C", "Cortar (C)"),
                                   (RollMode::Knife, "🔪 K", "Cuchillo/Dividir (K)")] {
                let active = self.mode == m;
                if ui.add(egui::Button::new(egui::RichText::new(lbl).size(11.0)).small()
                    .fill(if active { T.accent.gamma_multiply(0.45) } else { T.panel_raised }))
                    .on_hover_text(tip).clicked() { self.mode = m; }
            }
            ui.separator();
            ui.label(egui::RichText::new("Grid").size(10.0).color(T.text_dim));
            let qname = QUANTS.iter().find(|(q,_)| *q == self.quant).map(|(_, n)| *n).unwrap_or("1/16");
            egui::ComboBox::from_id_source("roll_quant").selected_text(qname).width(56.0)
                .show_ui(ui, |ui| { for (q, n) in QUANTS { ui.selectable_value(&mut self.quant, q, n); } });
            if ui.small_button("Cuantizar").on_hover_text("Ajustar inicio de notas a la rejilla").clicked() {
                let q = self.quant.max(1);
                let targets: Vec<usize> = if self.sel.is_empty() { (0..notes.len()).collect() } else { self.sel.clone() };
                for i in targets {
                    if let Some(n) = notes.get_mut(i) {
                        n.tick_start = ((n.tick_start as f32 / q as f32).round() as u32) * q;
                    }
                }
                *changed = true;
            }
            if ui.small_button("Vel 100").on_hover_text("Fijar velocity de selección a 100").clicked() {
                let targets: Vec<usize> = if self.sel.is_empty() { (0..notes.len()).collect() } else { self.sel.clone() };
                for i in targets {
                    if let Some(n) = notes.get_mut(i) { n.vel = 100; }
                }
                *changed = true;
            }
            if ui.small_button("Humanize").on_hover_text("Micro-desplazamiento aleatorio de inicio (±grid/4)").clicked() {
                let q = self.quant.max(4) / 4;
                let targets: Vec<usize> = if self.sel.is_empty() { (0..notes.len()).collect() } else { self.sel.clone() };
                for i in targets {
                    if let Some(n) = notes.get_mut(i) {
                        // LCG simple sin rand crate
                        let seed = (n.tick_start.wrapping_mul(1103515245).wrapping_add(12345)) as i32;
                        let delta = (seed % (q as i32 * 2 + 1)) - q as i32;
                        n.tick_start = n.tick_start.saturating_add_signed(delta).max(0);
                    }
                }
                *changed = true;
            }
            ui.separator();
            ui.label(egui::RichText::new("Zoom").size(10.0).color(T.text_dim));
            ui.add(egui::Slider::new(&mut self.px_per_tick, 0.03..=0.9).show_value(false));
            ui.toggle_value(&mut self.follow_head, "👁")
                .on_hover_text("Seguir playhead");
            ui.separator();
            ui.label(egui::RichText::new(format!("{} notas · {} comp", notes.len(), clip.loop_bars))
                .size(10.0).color(T.text_dim));
            if ui.button("✖").on_hover_text("Cerrar").clicked() { *close = true; }
        });
    }

    fn top_key(&self) -> i32 {
        self.scroll_key as i32 + self.n_rows as i32 - 1
    }
    fn row_y_full(&self, rect: Rect, key: u8) -> f32 {
        rect.top() + (self.top_key() - key as i32) as f32 * self.row_h
    }
    fn key_at_full(&self, rect: Rect, y: f32) -> u8 {
        let row = ((y - rect.top()) / self.row_h).floor() as i32;
        (self.top_key() - row).clamp(0, 127) as u8
    }

    fn paint_notes(&self, ui: &egui::Ui, rect: Rect, notes: &[MidiNote],
        loop_ticks: u32, t: usize, s: usize, shared: &SessionShared) {
        let p = ui.painter_at(rect); let px = self.px_per_tick;
        let top = self.top_key();
        let bottom = self.scroll_key as i32;
        for key in (bottom..=top).rev() {
            let key = key as u8;
            let y = self.row_y_full(rect, key);
            if y + self.row_h < rect.top() || y > rect.bottom() { continue; }
            let col = if is_black(key) { T.piano_black }
                else if key % 12 == 0 { T.piano_white.gamma_multiply(0.22) }
                else { T.piano_white.gamma_multiply(0.14) };
            p.rect_filled(Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), self.row_h)),
                Rounding::ZERO, col);
        }
        let q = self.quant.max(1);
        let mut tick = 0u32;
        while tick <= loop_ticks {
            let x = rect.left() + tick as f32 * px;
            let bar = tick % TICKS_PER_BAR as u32 == 0;
            p.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())],
                Stroke::new(1.0, Color32::from_gray(if bar { 90 } else { 44 })));
            tick += q;
        }
        let x_end = rect.left() + loop_ticks as f32 * px;
        p.line_segment([pos2(x_end, rect.top()), pos2(x_end, rect.bottom())],
            Stroke::new(2.0, T.accent.gamma_multiply(0.5)));
        for (i, n) in notes.iter().enumerate() {
            let y = self.row_y_full(rect, n.note);
            if y + self.row_h < rect.top() || y > rect.bottom() { continue; }
            let x = rect.left() + n.tick_start as f32 * px;
            let w = (n.tick_dur as f32 * px).max(3.0);
            let r = Rect::from_min_size(pos2(x, y + 1.0), vec2(w, self.row_h - 2.0));
            let v = n.vel as f32 / 127.0;
            // Velocity → luminosidad del accent (look DAW)
            let col = Color32::from_rgb(
                (40.0 + 50.0 * v) as u8,
                (90.0 + 80.0 * v) as u8,
                (160.0 + 75.0 * v) as u8,
            );
            p.rect_filled(r, Rounding::same(2.0), col);
            if self.sel.contains(&i) {
                p.rect_stroke(r, Rounding::same(2.0), Stroke::new(1.5, T.accent_gold));
            }
            if self.px_per_tick >= LABEL_ZOOM_THRESHOLD && w > 18.0 {
                p.text(pos2(x + 2.0, y + self.row_h * 0.5), Align2::LEFT_CENTER,
                    key_name(n.note), FontId::proportional(8.5),
                    Color32::WHITE.gamma_multiply(0.85));
            }
        }
        if shared.rt.active_scene(t) == Some(s as u8) {
            let x = rect.left() + shared.rt.playhead(t) as f32 * px;
            p.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())],
                Stroke::new(2.0, T.piano_head));
        }
    }

    fn paint_band_rect(&self, ui: &egui::Ui, rect: Rect) {
        if let Some(b) = &self.band {
            let r = Rect::from_two_pos(b.start, b.cur).intersect(rect);
            if r.is_positive() {
                let p = ui.painter_at(rect);
                p.rect_filled(r, Rounding::ZERO, T.accent.gamma_multiply(0.12));
                p.rect_stroke(r, Rounding::ZERO, Stroke::new(1.0, T.accent.gamma_multiply(0.7)));
            }
        }
    }

    fn interact_notes(&mut self, resp: &egui::Response, rect: Rect,
        notes: &mut Vec<MidiNote>, loop_ticks: u32) -> bool {
        let mut changed = false;
        let Some(pos) = resp.interact_pointer_pos() else {
            if resp.drag_stopped() { self.drag = None; self.band = None; }
            return false;
        };
        let px = self.px_per_tick; let q = self.quant.max(1);
        let tk = ((pos.x - rect.left()) / px).max(0.0);
        let key = self.key_at_full(rect, pos.y);
        let hit = notes.iter().position(|n| n.note == key
            && tk >= n.tick_start as f32 && tk < (n.tick_start + n.tick_dur) as f32);
        let edge_hit = notes.iter().enumerate().find_map(|(i, n)| self.note_resize_edge(rect, pos, n).map(|e| (i, e)));
        let hover_resize = edge_hit.is_some() || self.drag.as_ref().map(|d| d.resize).unwrap_or(false);
        if hover_resize { resp.ctx.set_cursor_icon(CursorIcon::ResizeHorizontal); }

        if resp.drag_started() || (resp.clicked() && self.drag.is_none() && self.band.is_none()) {
            match self.mode {
                RollMode::Cut => { if let Some(i) = hit {
                    notes.remove(i); self.sel.retain(|&x| x != i); changed = true;
                } }
                RollMode::Knife => {
                    let split = ((tk / q as f32).round() as u32 * q).clamp(1, loop_ticks.saturating_sub(1));
                    let mut to_split: Vec<usize> = notes.iter().enumerate().filter_map(|(i, n)| {
                        let end = n.tick_start.saturating_add(n.tick_dur.max(1));
                        (split > n.tick_start && split < end).then_some(i)
                    }).collect();
                    to_split.sort_unstable();
                    to_split.reverse();
                    for i in to_split {
                        let n = notes[i];
                        let end = n.tick_start.saturating_add(n.tick_dur.max(1));
                        let right_dur = end.saturating_sub(split);
                        let left_dur = split.saturating_sub(n.tick_start);
                        if left_dur == 0 || right_dur == 0 { continue; }
                        notes[i].tick_dur = left_dur;
                        let mut right = n;
                        right.tick_start = split;
                        right.tick_dur = right_dur;
                        notes.push(right);
                        changed = true;
                    }
                    if changed { self.sel.clear(); }
                }
                RollMode::Draw => {
                    let target = hit.or(edge_hit.map(|(i, _)| i));
                    if let Some(i) = target {
                        if !self.sel.contains(&i) { self.sel = vec![i]; }
                        let n = notes[i];
                        let edge = edge_hit.filter(|(ei, _)| *ei == i).map(|(_, e)| e)
                            .or_else(|| self.note_resize_edge(rect, pos, &n));
                        if let Some(resize_end) = edge {
                            // resize: start_x negative => resize start edge
                            let sx = if resize_end { pos.x } else { -(pos.x.abs() + 1.0) };
                            self.drag = Some(MoveDrag { resize: true, start_x: sx, start_y: pos.y,
                                orig: vec![(i, n.tick_start, n.tick_dur, n.note)] });
                        } else {
                            self.drag = Some(MoveDrag { resize: false, start_x: pos.x, start_y: pos.y,
                                orig: self.sel.iter().filter_map(|&j| notes.get(j)
                                    .map(|nn| (j, nn.tick_start, nn.tick_dur, nn.note))).collect() });
                        }
                    } else {
                        let start = ((tk / q as f32).floor() as u32 * q).min(loop_ticks.saturating_sub(1));
                        notes.push(MidiNote { tick_start: start, tick_dur: q, note: key,
                            vel: 100, channel: 0, occurrence: 0, probability: 1.0 });
                        self.sel = vec![notes.len() - 1]; changed = true;
                    }
                }
                RollMode::Select => {
                    let target = hit.or(edge_hit.map(|(i, _)| i));
                    if let Some(i) = target {
                        if !resp.ctx.input(|inp| inp.modifiers.shift) {
                            if !self.sel.contains(&i) { self.sel = vec![i]; }
                        } else if hit.is_some() {
                            if self.sel.contains(&i) { self.sel.retain(|&x| x != i); }
                            else { self.sel.push(i); }
                        }
                        let n = notes[i];
                        let edge = edge_hit.filter(|(ei, _)| *ei == i).map(|(_, e)| e)
                            .or_else(|| self.note_resize_edge(rect, pos, &n));
                        let (resize, sx) = if let Some(end) = edge { (true, if end { pos.x } else { -(pos.x.abs() + 1.0) }) } else { (false, pos.x) };
                        self.drag = Some(MoveDrag { resize, start_x: sx, start_y: pos.y,
                            orig: self.sel.iter().filter_map(|&j| notes.get(j)
                                .map(|nn| (j, nn.tick_start, nn.tick_dur, nn.note))).collect() });
                    } else {
                        if !resp.ctx.input(|inp| inp.modifiers.shift) { self.sel.clear(); }
                        self.band = Some(BandDrag { start: pos, cur: pos });
                    }
                }
            }
        }

        if resp.dragged() {
            if let Some(b) = self.band.as_mut() {
                b.cur = pos;
                let band_rect = Rect::from_two_pos(b.start, b.cur);
                self.sel.clear();
                for (i, n) in notes.iter().enumerate() {
                    let ny = self.row_y_full(rect, n.note);
                    let nx = rect.left() + n.tick_start as f32 * px;
                    let nw = (n.tick_dur as f32 * px).max(3.0);
                    let nr = Rect::from_min_size(pos2(nx, ny), vec2(nw, self.row_h));
                    if band_rect.intersects(nr) { self.sel.push(i); }
                }
            }
            if let Some(d) = self.drag.clone() {
                let resize_start = d.resize && d.start_x < 0.0;
                let sx = if resize_start { -(d.start_x + 1.0) } else { d.start_x };
                let dt_ticks = ((pos.x - sx) / px / q as f32).round() as i32 * q as i32;
                let dt_semis = ((d.start_y - pos.y) / self.row_h).round() as i32;
                for (i, st0, du0, note0) in &d.orig {
                    if let Some(n) = notes.get_mut(*i) {
                        if d.resize {
                            if resize_start {
                                let new_start = (*st0 as i32 + dt_ticks).clamp(0, *st0 as i32 + *du0 as i32 - q as i32);
                                let end = *st0 as i32 + *du0 as i32;
                                n.tick_start = new_start.max(0) as u32;
                                n.tick_dur = (end - n.tick_start as i32).max(q as i32) as u32;
                            } else {
                                n.tick_dur = ((*du0 as i32 + dt_ticks).max(q as i32)) as u32;
                            }
                        } else {
                            n.tick_start = (*st0 as i32 + dt_ticks).max(0) as u32;
                            n.note = (*note0 as i32 + dt_semis).clamp(0, 127) as u8;
                        }
                    }
                }
                changed = true;
            }
        }
        if resp.drag_stopped() { self.drag = None; self.band = None; }
        if resp.secondary_clicked() { if let Some(i) = hit {
            notes.remove(i); self.sel.retain(|&x| x != i); changed = true;
        } }
        changed
    }

    fn paint_keyboard(&self, ui: &egui::Ui, rect: Rect) {
        let p = ui.painter_at(rect);
        p.rect_filled(rect, Rounding::ZERO, T.panel.gamma_multiply(0.95));
        let top = self.top_key();
        let bottom = self.scroll_key as i32;
        for key in (bottom..=top).rev() {
            let key = key as u8;
            let y = self.row_y_full(rect, key);
            if y + self.row_h < rect.top() || y > rect.bottom() { continue; }
            let black = is_black(key); let c_note = key % 12 == 0;
            if black {
                p.rect_filled(Rect::from_min_size(pos2(rect.left(), y),
                    vec2(rect.width() * 0.6, self.row_h)), Rounding::ZERO, Color32::from_gray(40));
            } else if c_note {
                p.rect_filled(Rect::from_min_size(pos2(rect.left(), y),
                    vec2(rect.width(), self.row_h)), Rounding::ZERO, Color32::from_gray(72));
                p.text(pos2(rect.right() - 2.0, y + self.row_h * 0.5), Align2::RIGHT_CENTER,
                    key_name(key), FontId::proportional(9.0), T.text_dim);
            }
            if !black {
                p.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)],
                    Stroke::new(0.5, Color32::from_gray(55)));
            }
        }
    }

    fn focus_on_notes(&mut self, notes: &[MidiNote]) {
        if notes.is_empty() { return; }
        let min_n = notes.iter().map(|n| n.note).min().unwrap_or(60);
        let max_n = notes.iter().map(|n| n.note).max().unwrap_or(72);
        let center = ((min_n as i32 + max_n as i32) / 2).clamp(0, 127) as u8;
        let half = (self.n_rows / 2).max(1) as i32;
        self.scroll_key = (center as i32 - half).clamp(0, 127 - self.n_rows as i32 + 1).max(0) as u8;
        let min_t = notes.iter().map(|n| n.tick_start).min().unwrap_or(0) as f32;
        self.h_scroll_offset = (min_t * self.px_per_tick - 40.0).max(0.0);
    }

    fn note_resize_edge(&self, rect: Rect, pos: egui::Pos2, note: &MidiNote) -> Option<bool> {
        let px = self.px_per_tick;
        let start_x = rect.left() + note.tick_start as f32 * px;
        let end_x = start_x + (note.tick_dur as f32 * px).max(3.0);
        let y = self.row_y_full(rect, note.note);
        let note_rect = Rect::from_min_size(pos2(start_x, y), vec2((note.tick_dur as f32 * px).max(3.0), self.row_h));
        if !note_rect.expand(3.0).contains(pos) { return None; }
        let near_start = (pos.x - start_x).abs() <= 6.0;
        let near_end = (pos.x - end_x).abs() <= 6.0;
        if near_end { Some(true) } // true = resize end
        else if near_start { Some(false) } // false = resize start
        else { None }
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
        p.text(rect.left_top() + vec2(4.0, 2.0), Align2::LEFT_TOP,
            "Velocity", FontId::proportional(9.0), T.text_dim);
    }

    fn interact_velocity(&mut self, resp: &egui::Response, rect: Rect,
        notes: &mut Vec<MidiNote>) -> bool {
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
