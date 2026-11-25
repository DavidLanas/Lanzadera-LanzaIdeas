// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! Widgets custom estilo consola Vibez: knob con arco, fader vertical con
//! meter LED al lado. Todo con Painter (canvas), sin imágenes.
use egui::{pos2, vec2, Align2, Color32, FontId, Rect, Rounding, Sense, Stroke};
use crate::theme::T;

/// Knob de arrastre vertical. Doble click → valor por defecto.
pub fn knob(ui: &mut egui::Ui, value: &mut f32, min: f32, max: f32, default: f32, label: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(40.0, 50.0), Sense::click_and_drag());
    let mut changed = false;
    if resp.dragged() {
        *value = (*value - resp.drag_delta().y * (max - min) / 150.0).clamp(min, max);
        changed = true;
    }
    if resp.double_clicked() { *value = default; changed = true; }

    let c = rect.center() - vec2(0.0, 6.0);
    let r = 13.0;
    let p = ui.painter();
    p.circle_filled(c, r, T.knob_bg);
    p.circle_stroke(c, r, Stroke::new(1.0, Color32::from_gray(70)));
    let t = ((*value - min) / (max - min)).clamp(0.0, 1.0);
    let mut pts = Vec::new();
    let steps = 24;
    for k in 0..=steps {
        let a = (-225.0 + 270.0 * t * k as f32 / steps as f32).to_radians();
        pts.push(c + vec2(a.cos(), a.sin()) * (r - 2.0));
    }
    p.add(egui::Shape::line(pts, Stroke::new(2.5, T.knob_arc)));
    let ang = (-225.0 + 270.0 * t).to_radians();
    p.line_segment([c, c + vec2(ang.cos(), ang.sin()) * (r - 4.0)], Stroke::new(2.0, Color32::WHITE));
    p.text(rect.center_bottom(), Align2::CENTER_BOTTOM, label, FontId::proportional(10.0), T.text_dim);
    changed
}

/// Fader vertical + meter LED. Click fija el valor; arrastrar lo mueve.
pub fn fader(ui: &mut egui::Ui, value: &mut f32, height: f32, level: f32) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(34.0, height), Sense::click_and_drag());
    let mut changed = false;
    if resp.dragged() {
        *value = (*value - resp.drag_delta().y / height).clamp(0.0, 1.0);
        changed = true;
    }
    if resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            *value = ((rect.bottom() - p.y) / height).clamp(0.0, 1.0);
            changed = true;
        }
    }
    let track = Rect::from_min_size(rect.min + vec2(6.0, 0.0), vec2(10.0, height));
    let p = ui.painter();
    p.rect_filled(track, Rounding::same(3.0), T.fader_track);
    let y = rect.bottom() - *value * height;
    p.rect_filled(
        Rect::from_min_max(pos2(track.min.x, y), track.max),
        Rounding::same(3.0),
        Color32::from_rgba_unmultiplied(90, 160, 220, 60),
    );
    let thumb = Rect::from_center_size(pos2(track.center().x, y), vec2(22.0, 8.0));
    p.rect_filled(thumb, Rounding::same(2.0), T.fader_thumb);

    // Meter LED (verde → amarillo → rojo).
    let m = Rect::from_min_size(rect.min + vec2(24.0, 0.0), vec2(6.0, height));
    p.rect_filled(m, Rounding::same(2.0), Color32::from_gray(24));
    let lv = level.clamp(0.0, 1.0).sqrt();
    let mh = lv * height;
    let col = if lv > 0.85 { T.meter_high } else if lv > 0.55 { T.meter_mid } else { T.meter_low };
    if mh > 0.5 {
        p.rect_filled(Rect::from_min_max(pos2(m.min.x, m.bottom() - mh), m.max), Rounding::same(2.0), col);
    }
    changed
}
