//! Ventana flotante de plugin (Fase 3H).
//!
//! Muestra los parámetros de un plugin CLAP/VST3 en una ventana egui independiente
//! por pista, con buscador y botón para abrir la GUI nativa del plugin.
//!
//! # Atribución
//! Lógica de señalización de GUI nativa inspirada en el patrón de ventanas
//! flotantes de LMMS (GPL-2+) y Lemon Studio (MIT). Los widgets egui son propios.
//!
//! La GUI nativa CLAP usa la extensión `clap_plugin_gui` vía `clack-extensions 0.2`
//! (MIT/Apache-2.0). En modo flotante (`is_floating = true`), el plugin crea su
//! propia ventana OS sin necesitar un XID de padre — compatible con X11 y Wayland.

use egui::{Color32, FontId, RichText, Slider, Stroke};
use lanzadera_engine::shared::SessionShared;
use crate::theme::T;

/// Estado de la ventana de plugin para una pista concreta.
#[derive(Default)]
pub struct PluginParamWindow {
    pub track: usize,
    pub open: bool,
    pub search: String,
    pub native_requested: bool,
}

impl PluginParamWindow {
    pub fn new(track: usize) -> Self {
        Self { track, open: false, search: String::new(), native_requested: false }
    }
}

/// Conjunto de ventanas de plugin (una por pista, se expande bajo demanda).
pub struct PluginWindows {
    pub windows: Vec<PluginParamWindow>,
}

impl Default for PluginWindows {
    fn default() -> Self { Self { windows: Vec::new() } }
}

impl PluginWindows {
    pub fn open(&mut self, t: usize) {
        while self.windows.len() <= t {
            self.windows.push(PluginParamWindow::new(self.windows.len()));
        }
        self.windows[t].open = true;
        self.windows[t].native_requested = false;
    }

    pub fn show(&mut self, ctx: &egui::Context, shared: &SessionShared) -> Vec<usize> {
        let mut native_requests = Vec::new();
        for w in self.windows.iter_mut() {
            if !w.open { continue; }
            if let Some(r) = show_one(ctx, w, shared) {
                native_requests.push(r);
            }
        }
        native_requests
    }
}

fn show_one(ctx: &egui::Context, w: &mut PluginParamWindow, shared: &SessionShared) -> Option<usize> {
    let t = w.track;

    let (plugin_name, plugin_path, params) = {
        let st = shared.state.lock().unwrap();
        let tr = st.tracks.get(t)?;
        let name = tr.plugin_path.as_deref()
            .and_then(|p| p.rsplit('/').next())
            .unwrap_or("Plugin")
            .trim_end_matches(".clap")
            .trim_end_matches(".vst3")
            .to_string();
        let path = tr.plugin_path.clone().unwrap_or_default();
        let params = tr.plugin_params.clone();
        (name, path, params)
    };

    if plugin_path.is_empty() { w.open = false; return None; }

    let title = format!("🔌 {} — Pista {}", plugin_name, t + 1);
    let mut native_requested = None;
    let mut open = w.open;

    egui::Window::new(&title)
        .id(egui::Id::new(("plugin_window", t)))
        .open(&mut open)
        .default_size([340.0, 480.0])
        .min_size([260.0, 200.0])
        .resizable(true)
        .collapsible(true)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&plugin_name).strong().size(13.0).color(T.accent));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if lanzadera_engine::CLAP_ENABLED {
                        let btn = egui::Button::new("🖥 GUI nativa")
                            .fill(T.panel.gamma_multiply(1.2))
                            .stroke(Stroke::new(1.0, T.accent.gamma_multiply(0.6)));
                        if ui.add(btn).on_hover_text(
                            "Abre la interfaz gráfica propia del plugin (ventana flotante).\n\
                             Requiere que el plugin soporte el modo flotante CLAP GUI."
                        ).clicked() {
                            native_requested = Some(t);
                        }
                    }
                });
            });
            ui.label(RichText::new(trunc_path(&plugin_path, 48)).size(9.0).color(T.text_dim));
            ui.separator();

            if params.is_empty() {
                ui.label(RichText::new("Sin parámetros expuestos por este plugin.")
                    .size(11.0).color(T.text_dim));
                return;
            }

            ui.horizontal(|ui| {
                ui.label(RichText::new("🔍").size(11.0));
                ui.text_edit_singleline(&mut w.search);
                if ui.small_button("✖").on_hover_text("Limpiar").clicked() { w.search.clear(); }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let visible = if w.search.is_empty() { params.len() } else {
                        params.iter().filter(|(_, n, _)| filter_match(n, &w.search)).count()
                    };
                    ui.label(RichText::new(format!("{}/{}", visible, params.len()))
                        .size(9.0).color(T.text_dim));
                });
            });
            ui.add_space(2.0);

            let filter = w.search.to_lowercase();
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .max_height(360.0)
                .show(ui, |ui| {
                    egui::Grid::new(("plugin_params_grid", t))
                        .num_columns(3)
                        .spacing([6.0, 3.0])
                        .min_col_width(0.0)
                        .show(ui, |ui| {
                            for (idx, (_id, name, value)) in params.iter().enumerate() {
                                if !filter.is_empty() && !filter_match(name, &filter) { continue; }
                                ui.label(RichText::new(trunc(name, 18)).size(10.0).color(T.text_dim));
                                let mut v = *value;
                                let slider = Slider::new(&mut v, 0.0_f32..=1.0)
                                    .show_value(false);
                                    
                                if ui.add_sized([160.0, 16.0], slider).changed() {
                                    shared.queue_plugin_param(t, idx, v);
                                }
                                ui.label(RichText::new(format!("{:.3}", v))
                                    .size(9.0).color(T.text_dim).font(FontId::monospace(9.0)));
                                if ui.small_button("↺").on_hover_text("Resetear").clicked() {
                                    shared.queue_plugin_param(t, idx, 0.5);
                                }
                                ui.end_row();
                            }
                        });
                });
            ui.separator();

            ui.horizontal(|ui| {
                let ext = plugin_path.rsplit('.').next().unwrap_or("");
                let (badge_text, badge_color) = match ext {
                    "clap" => ("CLAP", Color32::from_rgb(80, 160, 230)),
                    "vst3" => ("VST3", Color32::from_rgb(200, 120, 60)),
                    _      => ("PLUGIN", T.text_dim),
                };
                let badge = RichText::new(badge_text)
                    .size(9.0).color(badge_color)
                    .background_color(badge_color.gamma_multiply(0.18));
                ui.label(badge);
                ui.label(RichText::new(format!("{} parámetros", params.len()))
                    .size(9.0).color(T.text_dim));
            });
        });

    w.open = open;
    native_requested
}

fn trunc(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_string() }
    else { format!("{}…", &s[..s.char_indices().nth(max.saturating_sub(1)).map(|(i,_)|i).unwrap_or(s.len())]) }
}

fn trunc_path(s: &str, max: usize) -> String {
    if s.len() <= max { return s.to_string(); }
    format!("…{}", &s[s.len().saturating_sub(max)..])
}

fn filter_match(name: &str, filter: &str) -> bool {
    name.to_lowercase().contains(filter)
}
