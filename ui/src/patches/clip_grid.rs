// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
use egui::{vec2, Align2, Color32, FontId, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, Ui, Vec2};
use super::theme_tokens as theme;
use super::session_snapshot::*;

/// Payload para arrastrar un clip entre celdas del grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipDragPayload {
    pub track: usize,
    pub scene: usize,
}

#[derive(Default, Debug)]
pub struct ClipGridActions {
    pub toggle_clip: Option<(usize, usize)>,
    pub launch_clip: Option<(usize, usize)>,
    pub select_clip: Option<(usize, usize)>,
    /// Crear clip vacío de 8 compases (sin iniciar grabación).
    pub create_clip: Option<(usize, usize)>,
    /// Crear clip vacío en pista armada e iniciar grabación.
    pub create_and_record_clip: Option<(usize, usize)>,
    pub delete_clip: Option<(usize, usize)>,
    pub duplicate_clip: Option<(usize, usize)>,
    pub copy_clip: Option<(usize, usize)>,
    pub cut_clip: Option<(usize, usize)>,
    pub paste_clip: Option<(usize, usize)>,
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
    pub add_scene: bool,
    pub remove_scene: Option<usize>,
    /// Mover/copiar clip: (src_track, src_scene, dst_track, dst_scene, copy)
    pub move_clip: Option<(usize, usize, usize, usize, bool)>,
    /// Exportar Groove Machine como MIDI a una escena del clip launch.
    pub export_groove_to_scene: Option<usize>,
}

pub fn render_session_matrix(ui: &mut Ui, s: &SessionSnapshot, a: &mut ClipGridActions) {
    render_top_bar(ui, &s.transport, a);
    ui.separator();

    let n_tracks = s.tracks.len() + usize::from(s.groove.is_some());
    let n_tracks = n_tracks.max(1);
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
                    let scene_name = s.scene_names.get(si).map(|n| n.as_str()).unwrap_or("");
                    scene_header(ui, si, scene_name, cell_w, scene_header_h, a);
                }
                ui.end_row();

                for (ti, track) in s.tracks.iter().enumerate() {
                    let is_selected = ti == s.selected_track;
                    track_header(ui, track, ti, header_w, cell_h, is_selected, a);
                    for si in 0..n_scenes {
                        let clip = track.clips.get(si).and_then(|c| c.as_ref());
                        clip_cell(ui, track, clip, ti, si, cell_w, cell_h, is_selected, a);
                    }
                    ui.end_row();
                }

                if let Some(g) = &s.groove {
                    groove_track_header(ui, &g.name, header_w, cell_h);
                    for si in 0..n_scenes {
                        groove_cell(ui, g.clips.get(si).and_then(|c| c.as_ref()), si, cell_w, cell_h, a);
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
        if ui.button("+ Escena").on_hover_text("Añadir escena al final").clicked() { a.add_scene = true; }
        if t.rec_queued {
            let c = 4_i32 - (t.beat_in_bar as i32 % 4);
            ui.label(RichText::new(format!("● REC {}", c)).color(theme::RED).strong());
        }
        if ui.button("■ Stop All").clicked() { a.stop_all = true; }
    });
}

fn scene_header(ui: &mut Ui, si: usize, scene_name: &str, w: f32, h: f32, a: &mut ClipGridActions) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
    ui.painter().rect_filled(rect, 4.0, theme::SCENE_COL_BG);
    ui.painter().rect_stroke(rect, 4.0, Stroke::new(1.0, theme::GRID_LINE));

    let label = if scene_name.trim().is_empty() {
        format!("Escena {}", si + 1)
    } else {
        scene_name.to_string()
    };
    ui.painter().text(
        rect.left_center() + Vec2::new(6.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(theme::FONT_BASE),
        theme::TEXT_COLOR,
    );

    let btn_rect = Rect::from_min_size(rect.right_center() - Vec2::new(24.0, 10.0), Vec2::new(18.0, 18.0));
    ui.painter().rect_filled(btn_rect, 3.0, Color32::from_rgb(52, 56, 66));
    ui.painter().rect_stroke(btn_rect, 3.0, Stroke::new(1.0, theme::GRID_LINE));
    ui.painter().text(btn_rect.center(), Align2::CENTER_CENTER, "▶", FontId::proportional(11.0), theme::TEXT_COLOR);

    if resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            if btn_rect.contains(pos) {
                a.launch_scene = Some(si);
            }
        }
    }

    if resp.hovered() {
        ui.painter().rect_filled(rect, 4.0, Color32::from_rgba_unmultiplied(63, 212, 90, 18));
    }
    resp.context_menu(|ui| {
        if ui.button("🗑 Eliminar escena").clicked() { a.remove_scene = Some(si); ui.close_menu(); }
        if ui.button("＋ Añadir escena").clicked() { a.add_scene = true; ui.close_menu(); }
    });
}

fn track_header(ui: &mut Ui, t: &TrackSnapshot, track_idx: usize, w: f32, h: f32, is_selected: bool, a: &mut ClipGridActions) {
    let (outer, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
    let p = ui.painter();

    p.rect_filled(outer, 2.0, theme::TRACK_HEADER_BG);
    p.rect_filled(Rect::from_min_size(outer.min, Vec2::new(5.0, h)), 2.0, t.color);
    p.rect_stroke(
        outer.shrink(0.5),
        2.0,
        Stroke::new(
            if is_selected { 1.5 } else { 1.0 },
            if is_selected { crate::theme::T.accent_gold } else { theme::GRID_LINE },
        ),
    );

    p.text(
        Pos2::new(outer.left() + 10.0, outer.top() + 8.0),
        Align2::LEFT_TOP,
        &t.name,
        FontId::proportional(theme::FONT_MEDIUM),
        theme::TEXT_COLOR,
    );

    let m = Rect::from_min_size(Pos2::new(outer.left() + 10.0, outer.bottom() - 8.0), Vec2::new(w - 16.0, 4.0));
    p.rect_filled(m, 1.0, theme::METER_BG);
    let meter = t.meter_level.clamp(0.0, 1.0);
    let meter_col = if meter > 0.85 { crate::theme::T.meter_high } else { theme::METER_FILL };
    p.rect_filled(Rect::from_min_size(m.min, Vec2::new(m.width() * meter, 4.0)), 1.0, meter_col);

    let bw = 22.0;
    let bh = 18.0;
    let y = outer.top() + 26.0;
    let x0 = outer.left() + 10.0;

    let arm = Rect::from_min_size(Pos2::new(x0, y), Vec2::new(bw, bh));
    let mute = arm.translate(Vec2::new(bw + 3.0, 0.0));
    let solo = mute.translate(Vec2::new(bw + 3.0, 0.0));
    let stop = solo.translate(Vec2::new(bw + 3.0, 0.0));

    for (r, label, active, col) in [
        (arm,  "●", t.armed,  Color32::from_rgb(232, 92, 92)),
        (mute, "M", t.muted,  Color32::from_rgb(240, 200, 70)),
        (solo, "S", t.soloed, Color32::from_rgb(240, 152, 72)),
    ] {
        let fill = if active { col } else { Color32::from_rgb(52, 56, 66) };
        let txt = if active && label == "M" { Color32::from_rgb(30, 30, 30) } else { theme::TEXT_COLOR };
        p.rect_filled(r, 3.0, fill);
        p.rect_stroke(r, 3.0, Stroke::new(1.0, theme::GRID_LINE));
        p.text(r.center(), Align2::CENTER_CENTER, label, FontId::proportional(10.0), txt);
    }
    p.rect_filled(stop, 3.0, Color32::from_rgb(52, 56, 66));
    p.rect_stroke(stop, 3.0, Stroke::new(1.0, theme::GRID_LINE));
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

fn groove_track_header(ui: &mut Ui, name: &str, w: f32, h: f32) {
    let (outer, _resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    let p = ui.painter();
    let accent = Color32::from_rgb(240, 170, 60);
    p.rect_filled(outer, 2.0, theme::TRACK_HEADER_BG);
    p.rect_filled(Rect::from_min_size(outer.min, Vec2::new(5.0, h)), 2.0, accent);
    p.rect_stroke(outer.shrink(0.5), 2.0, Stroke::new(1.0, theme::GRID_LINE));
    p.text(
        Pos2::new(outer.left() + 10.0, outer.top() + 10.0),
        Align2::LEFT_TOP,
        name,
        FontId::proportional(theme::FONT_MEDIUM),
        theme::TEXT_COLOR,
    );
    p.text(
        Pos2::new(outer.left() + 10.0, outer.top() + 30.0),
        Align2::LEFT_TOP,
        "Groove Machine",
        FontId::proportional(theme::FONT_SMALL),
        theme::BADGE_TEXT,
    );
}

fn groove_cell(ui: &mut Ui, c: Option<&ClipSnapshot>, si: usize, w: f32, h: f32, a: &mut ClipGridActions) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
    let bg = if c.is_some() { Color32::from_rgb(58, 46, 28) } else { Color32::from_rgb(24, 26, 31) };
    ui.painter().rect_filled(rect, 4.0, theme::CLIP_BORDER);
    ui.painter().rect_filled(rect.shrink(1.0), 4.0, bg);
    let label = c.map(|x| x.name.as_str()).unwrap_or("Exportar MIDI");
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label,
        FontId::proportional(theme::FONT_SMALL), theme::TEXT_COLOR);
    if resp.hovered() {
        ui.painter().rect_filled(rect.shrink(0.5), 4.0, Color32::from_rgba_unmultiplied(240, 170, 60, 28));
    }
    resp.context_menu(|ui| {
        if ui.button("Exportar Groove a esta escena").clicked() {
            a.export_groove_to_scene = Some(si);
            ui.close_menu();
        }
    });
    if resp.double_clicked() {
        a.export_groove_to_scene = Some(si);
    }
}

fn clip_cell(ui: &mut Ui, t: &TrackSnapshot, c: Option<&ClipSnapshot>, ti: usize, si: usize, w: f32, h: f32, is_selected_track: bool, a: &mut ClipGridActions) {
    let has_clip = c.is_some();
    let st = c.map(|x| x.state).unwrap_or(ClipStateView::Empty);
    let drag_id = egui::Id::new(("clip_drag", ti, si));
    let payload = ClipDragPayload { track: ti, scene: si };
    let is_drag_source = ui.ctx().is_being_dragged(drag_id);

    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click_and_drag());

    let track_tint = Color32::from_rgb(t.color[0], t.color[1], t.color[2]);
    let bg = if !has_clip {
        // Celda vacía: tinte sutil del color de pista sobre fondo oscuro.
        Color32::from_rgb(
            24u8.saturating_add((track_tint.r() as u16 / 14) as u8),
            26u8.saturating_add((track_tint.g() as u16 / 14) as u8),
            31u8.saturating_add((track_tint.b() as u16 / 14) as u8),
        )
    } else {
        let base = match st {
            ClipStateView::Empty => theme::CLIP_BG_STOPPED,
            ClipStateView::Playing => theme::CLIP_BG_PLAYING,
            ClipStateView::Triggered | ClipStateView::RecQueued => theme::CLIP_BG_TRIGGERED,
            ClipStateView::Recording | ClipStateView::Overdubbing => theme::CLIP_BG_RECORDING,
            ClipStateView::Stopping => theme::CLIP_BG_STOPPED,
        };
        // Mezcla el color de pista en el fondo del clip para identidad visual.
        Color32::from_rgb(
            ((base.r() as u16 * 72 + track_tint.r() as u16 * 28) / 100) as u8,
            ((base.g() as u16 * 72 + track_tint.g() as u16 * 28) / 100) as u8,
            ((base.b() as u16 * 72 + track_tint.b() as u16 * 28) / 100) as u8,
        )
    };

    // Fondo/borde base (el contenido se atenúa si estamos arrastrando esta celda)
    let alpha_mul = if is_drag_source { 0.35 } else { 1.0 };
    let paint = |col: Color32| -> Color32 {
        Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), ((col.a() as f32) * alpha_mul) as u8)
    };
    let border = if is_selected_track { crate::theme::T.accent_gold } else { track_tint.gamma_multiply(0.45) };
    let border_w = if is_selected_track { 1.5 } else { 1.0 };
    ui.painter().rect_filled(rect, 4.0, paint(border));
    ui.painter().rect_filled(rect.shrink(border_w), 4.0, paint(bg));
    // Banda fina de color de pista en el borde superior del clip.
    if has_clip {
        let stripe = Rect::from_min_size(rect.shrink(border_w).min, Vec2::new(rect.width() - border_w * 2.0, 3.0));
        ui.painter().rect_filled(stripe, 0.0, paint(track_tint.gamma_multiply(0.9)));
    }

    let btn = Rect::from_min_size(rect.min + Vec2::new(4.0, 4.0), Vec2::new(18.0, 18.0));
    const YELLOW: Color32 = Color32::from_rgb(240, 200, 70);
    const YELLOW_DIM: Color32 = Color32::from_rgb(110, 90, 30);
    const GREEN: Color32 = Color32::from_rgb(63, 212, 90);
    const REC_RED: Color32 = Color32::from_rgb(232, 70, 70);
    const IDLE: Color32 = Color32::from_rgb(141, 144, 152);
    let time = ui.input(|i| i.time) as f32;
    let blink_on = (time * 5.0).sin() > 0.0;
    let blinking = has_clip && matches!(st, ClipStateView::Triggered | ClipStateView::RecQueued);
    let (sym, col) = if !has_clip {
        (if t.armed { "●" } else { "■" }, if t.armed { REC_RED } else { IDLE })
    } else {
        match st {
            ClipStateView::Empty => ("■", IDLE),
            ClipStateView::Playing => ("▶", GREEN),
            ClipStateView::Stopping => ("■", Color32::from_rgb(240, 152, 72)),
            ClipStateView::Recording | ClipStateView::Overdubbing => ("●", REC_RED),
            ClipStateView::RecQueued => ("●", if blink_on { YELLOW } else { YELLOW_DIM }),
            ClipStateView::Triggered => ("▶", if blink_on { YELLOW } else { YELLOW_DIM }),
        }
    };
    if blinking { ui.ctx().request_repaint_after(std::time::Duration::from_millis(50)); }

    // Botón de estado: indicador + control de transporte/grabación.
    ui.painter().rect_filled(btn, 3.0, paint(Color32::from_rgb(46, 49, 58)));
    ui.painter().rect_stroke(btn, 3.0, Stroke::new(1.0, paint(theme::GRID_LINE)));
    ui.painter().text(btn.center(), Align2::CENTER_CENTER, sym, FontId::proportional(11.0), paint(col));

    if let Some(x) = c {
        ui.painter().text(
            rect.min + Vec2::new(26.0, 5.0),
            Align2::LEFT_TOP,
            &x.name,
            FontId::proportional(theme::FONT_BASE),
            paint(theme::TEXT_COLOR),
        );

        if !x.peaks.is_empty() {
            let wv_l = rect.left() + 24.0;
            let wv_t = rect.top() + 18.0;
            let wv_r = (rect.right() - 4.0).max(wv_l + 10.0);
            let wv_b = (rect.bottom() - 12.0).max(wv_t + 10.0);
            let wv = Rect::from_min_max(Pos2::new(wv_l, wv_t), Pos2::new(wv_r, wv_b));
            let wc = Color32::from_rgb(
                x.color.r().saturating_add(90),
                x.color.g().saturating_add(90),
                x.color.b().saturating_add(90),
            );
            let head = if x.progress > 0.0 { Some(x.progress) } else { None };
            crate::waveform::paint_cell(ui.painter(), wv, &x.peaks, head, paint(wc));
        }

        let pr = Rect::from_min_size(Pos2::new(rect.left() + 4.0, rect.bottom() - 10.0), Vec2::new(rect.width() - 8.0, 4.0));
        ui.painter().rect_filled(pr, 2.0, paint(theme::GRID_LINE));
        ui.painter().rect_filled(
            Rect::from_min_size(pr.min, Vec2::new(pr.width() * x.progress.clamp(0.0, 1.0), 4.0)),
            2.0, paint(theme::PROGRESS_FILL),
        );

        ui.painter().text(
            rect.right_top() + Vec2::new(-4.0, 4.0),
            Align2::RIGHT_TOP,
            format!("{}b Q{}", x.loop_bars, x.quantize_bars),
            FontId::monospace(9.0),
            paint(theme::BADGE_TEXT),
        );

        if let Some(icon) = x.follow_icon {
            ui.painter().text(
                rect.right_bottom() + Vec2::new(-4.0, -14.0),
                Align2::RIGHT_BOTTOM,
                icon,
                FontId::monospace(10.0),
                paint(theme::BADGE_TEXT),
            );
        }
    } else if t.armed {
        ui.painter().text(
            rect.left_center() + Vec2::new(6.0, 0.0),
            Align2::LEFT_CENTER,
            "●",
            FontId::proportional(theme::FONT_SMALL),
            paint(theme::RED),
        );
    }

    // Arrastre interno de clips (solo si la celda tiene clip)
    if has_clip && resp.drag_started_by(egui::PointerButton::Primary) {
        egui::DragAndDrop::set_payload(ui.ctx(), payload);
    }
    let dragging_clip = egui::DragAndDrop::has_payload_of_type::<ClipDragPayload>(ui.ctx());
    let dragging_file = egui::DragAndDrop::has_payload_of_type::<crate::view::BrowserFile>(ui.ctx());

    if resp.hovered() && !dragging_clip && !dragging_file {
        ui.painter().rect_filled(rect.shrink(0.5), 4.0, Color32::from_rgba_unmultiplied(63, 212, 90, 14));
    }

    // Destino de drop de clip interno
    if dragging_clip {
        let can_drop = ui.rect_contains_pointer(rect);
        if can_drop {
            let copy_mode = ui.input(|i| i.modifiers.alt);
            let stroke_col = if copy_mode { theme::PROGRESS_FILL } else { theme::TEXT_COLOR };
            ui.painter().rect_stroke(rect.shrink(1.0), 4.0, Stroke::new(2.0, stroke_col));
            if copy_mode {
                ui.painter().text(rect.center(), Align2::CENTER_CENTER, "⧉ Copiar", FontId::proportional(theme::FONT_SMALL), stroke_col);
            }
            if ui.input(|i| i.pointer.any_released()) {
                if let Some(src) = egui::DragAndDrop::take_payload::<ClipDragPayload>(ui.ctx()) {
                    if src.track != ti || src.scene != si {
                        a.move_clip = Some((src.track, src.scene, ti, si, copy_mode));
                    }
                }
            }
        }
    }

    // ── Interacciones (click/double/right) ──
    // El cuerpo del clip solo selecciona/enfoca. Play/rec se dispara desde el
    // botón de estado (esquina superior izquierda). Evita disparar al terminar drag.
    let clicked = resp.clicked() && !resp.dragged();
    let double_clicked = resp.double_clicked() && !resp.dragged();
    if double_clicked {
        if has_clip {
            a.open_piano_roll = Some((ti, si));
        } else {
            a.select_clip = Some((ti, si));
            a.create_clip = Some((ti, si));
        }
    } else if clicked {
        if let Some(pos) = resp.interact_pointer_pos() {
            if btn.contains(pos) {
                if has_clip {
                    a.toggle_clip = Some((ti, si));
                } else if t.armed {
                    a.create_and_record_clip = Some((ti, si));
                } else {
                    a.create_clip = Some((ti, si));
                }
            } else {
                a.select_clip = Some((ti, si));
            }
        }
    }

    resp.context_menu(|ui| {
        if has_clip && ui.button("🎹 Piano Roll").clicked() { a.open_piano_roll = Some((ti, si)); ui.close_menu(); }
        if !has_clip && ui.button("＋ Crear clip").clicked() { a.create_clip = Some((ti, si)); ui.close_menu(); }
        if ui.button("📂 Cargar audio/MIDI…").clicked() { a.open_audio_modal = Some((ti, si)); ui.close_menu(); }
        ui.separator();
        if has_clip && ui.button("Copiar").clicked() { a.copy_clip = Some((ti, si)); ui.close_menu(); }
        if has_clip && ui.button("Cortar").clicked() { a.cut_clip = Some((ti, si)); ui.close_menu(); }
        let can_paste = ui.ctx().data(|d| d.get_temp::<bool>(egui::Id::new("clip_clipboard_has")).unwrap_or(false));
        if ui.add_enabled(can_paste, egui::Button::new("Pegar")).clicked() { a.paste_clip = Some((ti, si)); ui.close_menu(); }
        if has_clip && ui.button("⧉ Duplicar").clicked() { a.duplicate_clip = Some((ti, si)); ui.close_menu(); }
        if has_clip && ui.button("🗑 Borrar clip").clicked() { a.delete_clip = Some((ti, si)); ui.close_menu(); }
        ui.separator();
        if ui.button("◎ Seleccionar").clicked() { a.select_clip = Some((ti, si)); ui.close_menu(); }
    });

    // Drop de archivos externos sobre la celda (browser/OS) o arrastre interno desde Browser.
    let dropped_files = ui.input(|i| i.raw.dropped_files.clone());
    let browser_hover = dragging_file && ui.rect_contains_pointer(rect);
    if browser_hover {
        ui.painter().rect_stroke(rect.shrink(1.0), 4.0, Stroke::new(2.0, theme::PROGRESS_FILL));
        if ui.input(|i| i.pointer.any_released()) {
            if let Some(payload) = egui::DragAndDrop::take_payload::<crate::view::BrowserFile>(ui.ctx()) {
                a.load_file = Some((ti, si, payload.0.clone()));
            }
        }
    }
    if resp.hovered() && !dropped_files.is_empty() {
        if let Some(f) = dropped_files.first() {
            if let Some(path) = &f.path {
                a.load_file = Some((ti, si, path.to_string_lossy().into_owned()));
            }
        }
    }
}

#[allow(dead_code)] // sustituida por waveform::paint_cell (C2b)
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
