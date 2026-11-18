//! Curva de EQ estilo ProQ3 simplificado: 3 bandas arrastrables sobre una
//! respuesta en frecuencia logarítmica (20 Hz – 20 kHz).
use egui::{pos2, vec2, Align2, Color32, FontId, Rect, Rounding, Sense, Stroke};
use crate::theme::T;

const F_MIN: f32 = 20.0;
const F_MAX: f32 = 20000.0;
const BANDS: [f32; 3] = [120.0, 1000.0, 6000.0];

fn x_of(f: f32, r: Rect) -> f32 {
    r.left() + r.width() * ((f / F_MIN).ln() / (F_MAX / F_MIN).ln())
}
fn y_of(db: f32, r: Rect) -> f32 {
    r.center().y - (db / 18.0) * (r.height() * 0.5 - 4.0)
}
fn gain_db(f: f32, g: [f32; 3]) -> f32 {
    let mut sum = 0.0;
    for (i, f0) in BANDS.iter().enumerate() {
        let l = (f / f0).ln();
        let w = if i == 1 { 0.7 } else { 0.85 };
        sum += g[i] * (-l * l / (2.0 * w * w)).exp();
    }
    sum
}

/// Dibuja la curva y permite arrastrar las 3 bandas en vertical.
pub fn curve(ui: &mut egui::Ui, lo: &mut f32, mid: &mut f32, hi: &mut f32) -> bool {
    let w = ui.available_width().max(180.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 92.0), Sense::click_and_drag());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, Rounding::same(4.0), T.fader_track);
    // Rejilla: 0 dB y décadas.
    p.line_segment([pos2(rect.left(), rect.center().y), pos2(rect.right(), rect.center().y)],
                   Stroke::new(1.0, Color32::from_gray(70)));
    for f in [100.0, 1000.0, 10000.0] {
        let x = x_of(f, rect);
        p.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.0, Color32::from_gray(45)));
        p.text(pos2(x, rect.bottom() - 2.0), Align2::CENTER_BOTTOM,
               if f >= 1000.0 { format!("{}k", f as i32 / 1000) } else { format!("{}", f as i32) },
               FontId::proportional(8.0), T.text_dim);
    }
    let g = [*lo, *mid, *hi];
    let mut pts = Vec::with_capacity(96);
    for i in 0..96 {
        let f = F_MIN * (F_MAX / F_MIN).powf(i as f32 / 95.0);
        pts.push(pos2(x_of(f, rect), y_of(gain_db(f, g), rect)));
    }
    p.add(egui::Shape::line(pts, Stroke::new(2.0, T.knob_arc)));
    // Handles de banda.
    for (i, f0) in BANDS.iter().enumerate() {
        let c = pos2(x_of(*f0, rect), y_of(g[i], rect));
        p.circle_filled(c, 4.5, T.accent);
        p.circle_stroke(c, 4.5, Stroke::new(1.0, Color32::WHITE));
    }
    let mut changed = false;
    if resp.dragged() {
        if let Some(pos) = resp.interact_pointer_pos() {
            // Banda cuyo handle esté más cerca en X.
            let mut best = 0usize;
            let mut bd = f32::MAX;
            for (i, f0) in BANDS.iter().enumerate() {
                let d = (x_of(*f0, rect) - pos.x).abs();
                if d < bd { bd = d; best = i; }
            }
            let db = ((rect.center().y - pos.y) / (rect.height() * 0.5 - 4.0) * 18.0).clamp(-15.0, 15.0);
            match best { 0 => *lo = db, 1 => *mid = db, _ => *hi = db }
            changed = true;
        }
    }
    if resp.double_clicked() { *lo = 0.0; *mid = 0.0; *hi = 0.0; changed = true; }
    changed
}
