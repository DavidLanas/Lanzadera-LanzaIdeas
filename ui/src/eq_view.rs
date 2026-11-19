//! Curva de EQ estilo ProQ3 simplificado: 3 bandas arrastrables sobre una
//! respuesta en frecuencia logarítmica (20 Hz – 20 kHz).
//!
//! Interacción ProQ-like:
//! - Drag vertical → gain
//! - Scroll de rueda sobre el handle del MID → Q (campana)
//! - Drag horizontal sobre handle → frecuencia (mid)
//! - Doble click → reset
use egui::{pos2, vec2, Align2, Color32, FontId, Rect, Rounding, Sense, Stroke};
use crate::theme::T;

const F_MIN: f32 = 20.0;
const F_MAX: f32 = 20000.0;

/// Estado de las 3 bandas (lo/mid/hi). Mid es peaking con Q variable.
#[derive(Clone, Copy)]
pub struct EqParams {
    pub lo: f32,      // dB
    pub mid: f32,     // dB
    pub hi: f32,      // dB
    pub mid_f: f32,   // Hz (default 1000)
    pub mid_q: f32,   // Q de la campana (default ~0.7)
}

impl Default for EqParams {
    fn default() -> Self {
        EqParams { lo: 0.0, mid: 0.0, hi: 0.0, mid_f: 1000.0, mid_q: 0.7 }
    }
}

fn x_of(f: f32, r: Rect) -> f32 {
    r.left() + r.width() * ((f / F_MIN).ln() / (F_MAX / F_MIN).ln())
}
fn y_of(db: f32, r: Rect) -> f32 {
    r.center().y - (db / 18.0) * (r.height() * 0.5 - 4.0)
}

fn gain_db(f: f32, p: EqParams) -> f32 {
    let mut sum = 0.0;
    // Low shelf approx (gaussian)
    let l = (f / 120.0).ln();
    sum += p.lo * (-l * l / (2.0 * 0.85 * 0.85)).exp();
    // Mid peaking
    let l = (f / p.mid_f.max(20.0)).ln();
    let w = (1.0 / p.mid_q.max(0.1)).clamp(0.15, 2.5);
    sum += p.mid * (-l * l / (2.0 * w * w)).exp();
    // High shelf
    let l = (f / 6000.0).ln();
    sum += p.hi * (-l * l / (2.0 * 0.85 * 0.85)).exp();
    sum
}

/// Dibuja la curva y permite editar gain (drag), Q (rueda en mid) y freq mid (drag horizontal).
/// Devuelve true si cambió algo. Los parámetros se mutan in-place.
pub fn curve_full(ui: &mut egui::Ui, p: &mut EqParams) -> bool {
    let w = ui.available_width().max(180.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 100.0), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, Rounding::same(4.0), T.fader_track);

    // Rejilla 0 dB + décadas
    painter.line_segment(
        [pos2(rect.left(), rect.center().y), pos2(rect.right(), rect.center().y)],
        Stroke::new(1.0, Color32::from_gray(70)),
    );
    for f in [100.0, 1000.0, 10000.0] {
        let x = x_of(f, rect);
        painter.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(1.0, Color32::from_gray(42)),
        );
        painter.text(
            pos2(x, rect.bottom() - 2.0),
            Align2::CENTER_BOTTOM,
            if f >= 1000.0 { format!("{}k", f as i32 / 1000) } else { format!("{}", f as i32) },
            FontId::proportional(8.0),
            T.text_dim,
        );
    }

    // Curva
    let mut pts = Vec::with_capacity(96);
    for i in 0..96 {
        let f = F_MIN * (F_MAX / F_MIN).powf(i as f32 / 95.0);
        pts.push(pos2(x_of(f, rect), y_of(gain_db(f, *p), rect)));
    }
    painter.add(egui::Shape::line(pts, Stroke::new(2.0, T.knob_arc)));

    // Handles: lo (120), mid (variable), hi (6k)
    let bands = [
        (120.0, p.lo, false),
        (p.mid_f, p.mid, true),
        (6000.0, p.hi, false),
    ];
    for (i, (f0, g, is_mid)) in bands.iter().enumerate() {
        let c = pos2(x_of(*f0, rect), y_of(*g, rect));
        let r = if *is_mid { 5.5 } else { 4.5 };
        painter.circle_filled(c, r, if *is_mid { T.accent_gold } else { T.accent });
        painter.circle_stroke(c, r, Stroke::new(1.0, Color32::WHITE));
        // Q visual: radio de la campana del mid
        if *is_mid {
            let q_vis = (1.0 / p.mid_q.max(0.15)).clamp(0.3, 3.0) * 8.0;
            painter.circle_stroke(c, q_vis, Stroke::new(1.0, Color32::from_rgba_unmultiplied(200, 165, 80, 90)));
        }
        let _ = i;
    }

    let mut changed = false;

    // Scroll sobre el área → Q del mid (estilo ProQ3)
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.1 {
            // Más cerca del handle mid → más efecto
            if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
                let mid_x = x_of(p.mid_f, rect);
                if (pos.x - mid_x).abs() < 40.0 || scroll.abs() > 2.0 {
                    // rueda arriba = Q más estrecho (más “campana”)
                    let factor = 1.0 + (-scroll * 0.004).clamp(-0.25, 0.25);
                    p.mid_q = (p.mid_q * factor).clamp(0.15, 8.0);
                    changed = true;
                }
            }
        }
    }

    if resp.dragged() {
        if let Some(pos) = resp.interact_pointer_pos() {
            // Banda más cercana en X
            let candidates = [120.0, p.mid_f, 6000.0];
            let mut best = 0usize;
            let mut bd = f32::MAX;
            for (i, f0) in candidates.iter().enumerate() {
                let d = (x_of(*f0, rect) - pos.x).abs();
                if d < bd {
                    bd = d;
                    best = i;
                }
            }
            let db = ((rect.center().y - pos.y) / (rect.height() * 0.5 - 4.0) * 18.0).clamp(-15.0, 15.0);
            match best {
                0 => p.lo = db,
                1 => {
                    p.mid = db;
                    // Drag horizontal mueve la frecuencia del mid
                    let rel = ((pos.x - rect.left()) / rect.width()).clamp(0.02, 0.98);
                    p.mid_f = F_MIN * (F_MAX / F_MIN).powf(rel);
                    p.mid_f = p.mid_f.clamp(80.0, 12000.0);
                }
                _ => p.hi = db,
            }
            changed = true;
        }
    }

    if resp.double_clicked() {
        *p = EqParams::default();
        changed = true;
    }

    // Tooltip / valores
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        let tip = format!(
            "Low {:+.1} dB  ·  Mid {:+.1} dB @ {:.0} Hz  Q={:.2}  ·  High {:+.1} dB\nRueda sobre MID = Q (campana)",
            p.lo, p.mid, p.mid_f, p.mid_q, p.hi
        );
        resp.on_hover_text(tip);
    }

    changed
}

/// Compatibilidad con la API antigua (solo 3 gains).
pub fn curve(ui: &mut egui::Ui, lo: &mut f32, mid: &mut f32, hi: &mut f32) -> bool {
    let mut p = EqParams {
        lo: *lo,
        mid: *mid,
        hi: *hi,
        mid_f: 1000.0,
        mid_q: 0.7,
    };
    let changed = curve_full(ui, &mut p);
    *lo = p.lo;
    *mid = p.mid;
    *hi = p.hi;
    changed
}
