//! Editor de muestra (lote C3): onda grande con zoom, seleccion y operaciones.
//! Reutiliza el modulo waveform; las operaciones viven en el motor (no en RT).
use std::sync::Arc;
use egui::{pos2, vec2, Color32, Rect, Sense, Stroke};
use lanzadera_engine::shared::SessionShared;
use crate::theme::T;
use crate::waveform;

pub struct SampleEditor {
    pub open: bool,
    pub target: Option<(usize, usize)>,
    pub zoom: f32,
    pub scroll: f32,
    /// (ancla, actual) en fraccion del archivo; sin ordenar.
    pub sel: Option<(f32, f32)>,
    pub gain_db: f32,
    pub msg: String,
}

impl Default for SampleEditor {
    fn default() -> Self {
        SampleEditor { open: false, target: None, zoom: 1.0, scroll: 0.0, sel: None, gain_db: 0.0, msg: String::new() }
    }
}

impl SampleEditor {
    pub fn open_for(&mut self, t: usize, s: usize) {
        self.open = true;
        self.target = Some((t, s));
        self.zoom = 1.0;
        self.scroll = 0.0;
        self.sel = None;
        self.msg.clear();
    }

    pub fn show(&mut self, ctx: &egui::Context, shared: &Arc<SessionShared>) {
        if !self.open { return; }
        let Some((t, s)) = self.target else { return };
        let mut open = true;
        let mut op = None;
        egui::Window::new("Editor de muestra")
            .open(&mut open)
            .default_size([900.0, 430.0])
            .min_size([520.0, 280.0])
            .resizable(true)
            .show(ctx, |ui| {
                let Some(sd) = shared.samples.load(t, s) else {
                    ui.label("Este clip no tiene audio cargado. Carga un WAV o graba una toma.");
                    return;
                };
                let ch = sd.channels.max(1) as usize;
                let frames = sd.data.len() / ch;
                if frames == 0 { ui.label("Muestra vacia."); return; }
                let secs = frames as f64 / sd.sample_rate.max(1) as f64;
                let loop_bars = {
                    let st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                    st.tracks.get(t).and_then(|tr| tr.clips.get(s)).and_then(|c| c.as_ref())
                        .map(|c| c.loop_bars.max(1)).unwrap_or(1)
                };
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("Pista {} · Escena {}", t + 1, s + 1))
                        .strong().color(T.accent_gold));
                    ui.label(egui::RichText::new(format!(
                        "{:.2} s · {} ch · {} Hz · loop {} compases", secs, ch, sd.sample_rate, loop_bars))
                        .size(11.0).color(T.text_dim));
                });
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.label("Zoom");
                    ui.add_sized(vec2(120.0, 20.0), egui::Slider::new(&mut self.zoom, 1.0..=64.0)
                        .logarithmic(true).show_value(false));
                    if ui.button("Ver todo").clicked() { self.zoom = 1.0; self.scroll = 0.0; }
                    ui.separator();
                    ui.label("Gain");
                    ui.add_sized(vec2(150.0, 20.0), egui::Slider::new(&mut self.gain_db, -24.0..=24.0)
                        .suffix(" dB"));
                    if ui.button("Aplicar").on_hover_text("Escribe un WAV nuevo y recarga el clip").clicked() {
                        op = Some(lanzadera_engine::sample_edit::SampleOp::Gain(
                            (10.0f32).powf(self.gain_db / 20.0)));
                    }
                    if ui.button("Normalizar").clicked() {
                        op = Some(lanzadera_engine::sample_edit::SampleOp::Normalize);
                    }
                    if ui.button("Reversa").clicked() {
                        op = Some(lanzadera_engine::sample_edit::SampleOp::Reverse);
                    }
                });
                ui.add_space(4.0);

                let zoom = self.zoom.clamp(1.0, 64.0);
                let span = 1.0 / zoom;
                self.scroll = self.scroll.clamp(0.0, (1.0 - span).max(0.0));
                let a = self.scroll;
                let b = (a + span).min(1.0);
                let w = ui.available_width().max(80.0);
                let (rect, resp) = ui.allocate_exact_size(vec2(w, 210.0), Sense::click_and_drag());

                // Rueda = zoom alrededor del puntero.
                if resp.hovered() {
                    let dy = ui.input(|i| i.raw_scroll_delta.y);
                    if dy.abs() > 0.5 {
                        let px = resp.hover_pos()
                            .map(|p| ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0))
                            .unwrap_or(0.5);
                        let focus = a + px * span;
                        let nz = (zoom * (1.0 + dy * 0.002)).clamp(1.0, 64.0);
                        let nspan = 1.0 / nz;
                        self.zoom = nz;
                        self.scroll = (focus - px * nspan).clamp(0.0, (1.0 - nspan).max(0.0));
                    }
                }
                let frac = |x: f32| (a + ((x - rect.left()) / rect.width()).clamp(0.0, 1.0) * span).clamp(0.0, 1.0);
                if resp.drag_started() {
                    if let Some(p) = resp.interact_pointer_pos() { let f = frac(p.x); self.sel = Some((f, f)); }
                }
                if resp.dragged() {
                    if let (Some(p), Some((s0, _))) = (resp.interact_pointer_pos(), self.sel) {
                        let f = frac(p.x);
                        self.sel = Some((s0, f));
                    }
                }
                if resp.double_clicked() { self.sel = None; }

                let f0 = ((a * frames as f32) as usize).min(frames - 1);
                let f1 = (((b * frames as f32) as usize).max(f0 + 1)).min(frames);
                let cols = (rect.width() as usize).clamp(1, 2048);
                let peaks = waveform::overview_from_samples(&sd.data[f0 * ch..f1 * ch], ch, cols);
                let lt = (loop_bars as u32 * lanzadera_core::TICKS_PER_BAR as u32).max(1);
                let head = if shared.rt.active_scene(t) == Some(s as u8) {
                    let p = shared.rt.playhead(t) as f32 / lt as f32;
                    if p >= a && p <= b { Some((p - a) / span) } else { None }
                } else { None };
                let p = ui.painter_at(rect);
                waveform::paint(&p, rect, &peaks, head, &waveform::WaveStyle::default());
                if let Some((s0, s1)) = self.sel {
                    let lo = s0.min(s1);
                    let hi = s0.max(s1);
                    let x0 = rect.left() + ((lo - a) / span).clamp(0.0, 1.0) * rect.width();
                    let x1 = rect.left() + ((hi - a) / span).clamp(0.0, 1.0) * rect.width();
                    if x1 > x0 {
                        p.rect_filled(Rect::from_min_max(pos2(x0, rect.top()), pos2(x1, rect.bottom())),
                            0.0, Color32::from_rgba_unmultiplied(120, 190, 255, 45));
                    }
                }
                for k in 0..=8 {
                    let f = a + (b - a) * k as f32 / 8.0;
                    let x = rect.left() + rect.width() * k as f32 / 8.0;
                    p.line_segment([pos2(x, rect.bottom() - 6.0), pos2(x, rect.bottom())],
                        Stroke::new(1.0, T.text_dim));
                    p.text(pos2(x + 2.0, rect.bottom() - 7.0), egui::Align2::LEFT_BOTTOM,
                        format!("{:.2}s", f * secs as f32), egui::FontId::proportional(9.0), T.text_dim);
                }
                if zoom > 1.01 {
                    ui.add(egui::Slider::new(&mut self.scroll, 0.0..=(1.0 - span).max(0.001))
                        .show_value(false));
                }
                if let Some((s0, s1)) = self.sel {
                    let lo = s0.min(s1) * secs as f32;
                    let hi = s0.max(s1) * secs as f32;
                    ui.label(egui::RichText::new(format!(
                        "Seleccion: {:.3} s → {:.3} s  ({:.3} s) · doble clic la limpia", lo, hi, hi - lo))
                        .size(10.0).color(T.text_dim));
                }
                if !self.msg.is_empty() {
                    ui.label(egui::RichText::new(&self.msg).size(10.0).color(T.accent));
                }
            });
        self.open = open;
        if let Some(o) = op {
            self.msg = match lanzadera_engine::sample_edit::apply(shared, t, s, o) {
                Ok(m) => format!("Aplicado: {}", m),
                Err(e) => format!("⚠ {}", e),
            };
        }
    }
}
