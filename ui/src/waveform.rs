// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! Vista de ondas de clips de audio (lote C2).
//!
//! Implementacion propia para Lanzadera: el concepto (overview diezmado por
//! columna de pixeles + playhead) es el habitual en cualquier DAW; no se
//! incorpora codigo de terceros. Los picos ya los calcula el loader
//! (SampleData::peaks), asi que dibujar no decodifica audio ni asigna memoria.
use egui::{pos2, vec2, Color32, Rect, Rounding, Sense, Stroke};
use crate::theme::T;

/// Colores del dibujo. `wave` se puede tenir con el color del clip.
pub struct WaveStyle {
    pub bg: Color32,
    pub wave: Color32,
    pub center: Color32,
    pub head: Color32,
}

impl Default for WaveStyle {
    fn default() -> Self {
        WaveStyle {
            bg: Color32::from_gray(18),
            wave: Color32::from_rgb(140, 195, 240),
            center: T.text_dim,
            head: Color32::WHITE,
        }
    }
}

/// Pinta la onda en `rect` a partir de picos absolutos (0..1).
///
/// Una columna por pixel: de los buckets que caen en esa columna se toma el
/// MAXIMO, asi no se pierden transitorios aunque el overview sea mas largo
/// que el ancho disponible. `head` = playhead 0..1 (None = clip parado).
pub fn paint(p: &egui::Painter, rect: Rect, peaks: &[f32], head: Option<f32>, st: &WaveStyle) {
    p.rect_filled(rect, Rounding::same(3.0), st.bg);
    let mid = rect.center().y;
    p.line_segment(
        [pos2(rect.left(), mid), pos2(rect.right(), mid)],
        Stroke::new(1.0, st.center),
    );
    if peaks.is_empty() || rect.width() < 2.0 {
        return;
    }
    let w = rect.width();
    let half = (rect.height() * 0.5 - 2.0).max(1.0);
    let cols = (w as usize).clamp(1, 4096);
    let n = peaks.len();
    for c in 0..cols {
        let i0 = c * n / cols;
        let i1 = (((c + 1) * n / cols).max(i0 + 1)).min(n);
        let mut pk = 0.0f32;
        for i in i0..i1 {
            if peaks[i] > pk { pk = peaks[i]; }
        }
        let h = (pk.clamp(0.0, 1.0) * half).max(0.5);
        let x = rect.left() + (c as f32 + 0.5) * w / cols as f32;
        p.line_segment([pos2(x, mid - h), pos2(x, mid + h)], Stroke::new(1.0, st.wave));
    }
    if let Some(f) = head {
        let x = rect.left() + f.clamp(0.0, 1.0) * w;
        p.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.5, st.head));
    }
}

/// Widget de altura fija (inspector, editor de samples). Devuelve la respuesta
/// para que el llamador pueda anadir interaccion (clic para localizar, etc.).
pub fn view(ui: &mut egui::Ui, peaks: &[f32], height: f32, head: Option<f32>) -> egui::Response {
    let w = ui.available_width().max(40.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, height), Sense::hover());
    let p = ui.painter_at(rect);
    paint(&p, rect, peaks, head, &WaveStyle::default());
    resp
}

/// Version compacta para una celda del grid (C2b): sin fondo propio y con la
/// onda tenida del color del clip.
#[allow(dead_code)]
pub fn paint_cell(p: &egui::Painter, rect: Rect, peaks: &[f32], head: Option<f32>, color: Color32) {
    let st = WaveStyle {
        bg: Color32::TRANSPARENT,
        wave: color,
        center: color.gamma_multiply(0.35),
        head: Color32::WHITE,
    };
    paint(p, rect, peaks, head, &st);
}

/// Overview de emergencia: picos desde las muestras crudas cuando el loader
/// no los publico (o estan a cero). Solo hilo de UI, nunca RT.
pub fn overview_from_samples(data: &[f32], channels: usize, buckets: usize) -> Vec<f32> {
    let ch = channels.max(1);
    let frames = data.len() / ch;
    let mut out = vec![0.0f32; buckets.max(1)];
    if frames == 0 { return out; }
    for b in 0..out.len() {
        let a = b * frames / out.len();
        let e = (((b + 1) * frames / out.len()).max(a + 1)).min(frames);
        let mut m = 0.0f32;
        for f in a..e {
            for c in 0..ch {
                let v = data[f * ch + c].abs();
                if v > m { m = v; }
            }
        }
        out[b] = m;
    }
    out
}
