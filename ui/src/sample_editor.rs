//! Editor de muestra (lote C3): onda grande con zoom, seleccion, operaciones
//! (gain / normalizar / reversa / silenciar / fades / recorte) y control del
//! loop (offset de inicio y longitud = loop parcial dentro del archivo).
//! Las operaciones viven en engine/src/sample_edit.rs y escriben un WAV nuevo:
//! el original no se toca.
use std::sync::Arc;
use egui::{pos2, vec2, Color32, Rect, Sense, Stroke};
use lanzadera_engine::sample_edit::SampleOp;
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
        let mut op: Option<SampleOp> = None;
        // (offset en segundos, barras opcionales) pedidos por los botones de loop.
        let mut loop_cmd: Option<(f32, Option<u8>)> = None;
        egui::Window::new("Editor de muestra")
            .open(&mut open)
            .default_size([920.0, 480.0])
            .min_size([540.0, 300.0])
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
                let (tempo, lstart, lbars) = {
                    let st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                    let tempo = st.tempo.max(20.0);
                    let (ls, lb) = st.tracks.get(t).and_then(|tr| tr.clips.get(s)).and_then(|c| c.as_ref())
                        .map(|c| (c.loop_start_sec, c.loop_bars.max(1))).unwrap_or((0.0, 1));
                    (tempo, ls, lb)
                };
                let bar_secs = 240.0 / tempo;
                let loop_secs = lbars as f64 * bar_secs;
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("Pista {} · Escena {}", t + 1, s + 1))
                        .strong().color(T.accent_gold));
                    ui.label(egui::RichText::new(format!("{:.2} s · {} ch · {} Hz", secs, ch, sd.sample_rate))
                        .size(11.0).color(T.text_dim));
                    ui.label(egui::RichText::new(format!(
                        "Loop: empieza en {:.2} s · {} comp. ({:.2} s)", lstart, lbars, loop_secs))
                        .size(11.0).color(T.text_dim));
                });
                ui.separator();
                // Fila 1: zoom + gain + operaciones globales.
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
                        op = Some(SampleOp::Gain((10.0f32).powf(self.gain_db / 20.0)));
                    }
                    if ui.button("Normalizar").clicked() { op = Some(SampleOp::Normalize); }
                    if ui.button("Reversa").clicked() { op = Some(SampleOp::Reverse); }
                });
                // Fila 2: operaciones sobre la selección + región de loop.
                let sel_range = self.sel.map(|(s0, s1)| (s0.min(s1), s0.max(s1)))
                    .filter(|(lo, hi)| hi - lo > 0.001);
                ui.horizontal_wrapped(|ui| {
                    ui.label("Selección:");
                    match sel_range {
                        Some((lo, hi)) => {
                            ui.label(egui::RichText::new(format!(
                                "{:.3} → {:.3} s", lo * secs as f32, hi * secs as f32))
                                .size(10.0).color(T.text_dim));
                            if ui.button("Silenciar").clicked() { op = Some(SampleOp::Silence(lo, hi)); }
                            if ui.button("Fade in").clicked() { op = Some(SampleOp::FadeIn(lo, hi)); }
                            if ui.button("Fade out").clicked() { op = Some(SampleOp::FadeOut(lo, hi)); }
                            if ui.button("✂ Recortar")
                                .on_hover_text("El archivo queda solo con la selección; el loop se reajusta y el offset vuelve a 0")
                                .clicked()
                            {
                                op = Some(SampleOp::Trim(lo, hi));
                                self.sel = None;
                            }
                            ui.separator();
                            if ui.button("⤓ Inicio loop = sel.")
                                .on_hover_text("El loop empieza donde empieza la selección: loop parcial dentro del archivo")
                                .clicked()
                            {
                                loop_cmd = Some((lo * secs as f32, None));
                            }
                            if ui.button("Loop = selección")
                                .on_hover_text("Offset y longitud del loop = selección (longitud redondeada a compases)")
                                .clicked()
                            {
                                let sel_secs = (hi - lo) as f64 * secs;
                                let bars = ((sel_secs / bar_secs).round().max(1.0) as i64).clamp(1, 255) as u8;
                                loop_cmd = Some((lo * secs as f32, Some(bars)));
                            }
                        }
                        None => {
                            ui.label(egui::RichText::new("arrastra sobre la onda para seleccionar")
                                .size(10.0).color(T.text_dim));
                        }
                    }
                    if lstart > 0.0 {
                        if ui.button("Offset 0").on_hover_text("El loop vuelve al principio del archivo").clicked() {
                            loop_cmd = Some((0.0, None));
                        }
                    }
                });
                ui.add_space(4.0);

                // ── Onda ──
                let zoom = self.zoom.clamp(1.0, 64.0);
                let span = 1.0 / zoom;
                self.scroll = self.scroll.clamp(0.0, (1.0 - span).max(0.0));
                let a = self.scroll;
                let b = (a + span).min(1.0);
                let w = ui.available_width().max(80.0);
                let (rect, resp) = ui.allocate_exact_size(vec2(w, 210.0), Sense::click_and_drag());
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
                // Playhead real, mapeado al offset del loop.
                let lt = (lbars as u32 * lanzadera_core::TICKS_PER_BAR as u32).max(1);
                let head = if shared.rt.active_scene(t) == Some(s as u8) {
                    let p = shared.rt.playhead(t) as f32 / lt as f32;
                    let file_pos = lstart as f64 + p as f64 * loop_secs;
                    let fr = (file_pos / secs).clamp(0.0, 1.0) as f32;
                    if fr >= a && fr <= b { Some((fr - a) / span) } else { None }
                } else { None };
                let p = ui.painter_at(rect);
                waveform::paint(&p, rect, &peaks, head, &waveform::WaveStyle::default());
                // Región del loop sombreada (offset + compases).
                let ls_f = (lstart as f64 / secs).clamp(0.0, 1.0) as f32;
                let le_f = ((lstart as f64 + loop_secs) / secs).clamp(0.0, 1.0) as f32;
                if le_f > a && ls_f < b {
                    let x0 = rect.left() + ((ls_f - a) / span).clamp(0.0, 1.0) * rect.width();
                    let x1 = rect.left() + ((le_f - a) / span).clamp(0.0, 1.0) * rect.width();
                    if x1 > x0 {
                        p.rect_filled(Rect::from_min_max(pos2(x0, rect.top()), pos2(x1, rect.bottom())),
                            0.0, Color32::from_rgba_unmultiplied(230, 190, 80, 26));
                        p.line_segment([pos2(x0, rect.top()), pos2(x0, rect.bottom())],
                            Stroke::new(1.5, T.accent_gold));
                        p.line_segment([pos2(x1, rect.top()), pos2(x1, rect.bottom())],
                            Stroke::new(1.0, T.accent_gold));
                    }
                }
                // Selección.
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
                // Regla de tiempo.
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
                        "Selección: {:.3} s → {:.3} s  ({:.3} s) · doble clic la limpia", lo, hi, hi - lo))
                        .size(10.0).color(T.text_dim));
                }
                if !self.msg.is_empty() {
                    ui.label(egui::RichText::new(&self.msg).size(10.0).color(T.accent));
                }
            });
        self.open = open;
        if let Some((start, bars)) = loop_cmd {
            shared.edit(|st| {
                if let Some(Some(c)) = st.tracks.get_mut(t).and_then(|tr| tr.clips.get_mut(s)) {
                    c.loop_start_sec = start;
                    if let Some(b) = bars { c.loop_bars = b; }
                }
            });
            self.msg = match bars {
                Some(b) => format!("Loop: empieza en {:.2} s · {} compases", start, b),
                None => format!("Inicio del loop: {:.2} s", start),
            };
        }
        if let Some(o) = op {
            self.msg = match lanzadera_engine::sample_edit::apply(shared, t, s, o) {
                Ok(m) => format!("Aplicado: {}", m),
                Err(e) => format!("⚠ {}", e),
            };
        }
    }
}
