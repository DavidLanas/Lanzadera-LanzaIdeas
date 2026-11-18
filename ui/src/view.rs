//! SessionView — layout principal de Fase 2 (spec §9.1):
//! TOP BAR / SESSION VIEW (grid) / MIXER (strips Vibez) / INSPECTOR derecho.
//! Regla de oro: NUNCA llamar a shared.edit/apply_control con el Mutex de
//! estado tomado; los widgets acumulan comandos y se aplican al final.
use std::sync::Arc;

use egui::{pos2, vec2, Align2, Color32, FontId, Rect, Rounding, Sense, Stroke, Pos2};
use lanzadera_core::control::ControlCmd;
use lanzadera_core::{Clip, ClipState, FollowAction, LaunchMode, Quantize, MAX_TRACKS, TICKS_PER_BAR};
use lanzadera_engine::shared::{LoadStatus, SessionShared};

use crate::theme::T;
use crate::widgets;
use crate::piano_roll::PianoRoll;
use crate::eq_view;

const Q_LIST: [(Quantize, &str); 10] = [
    (Quantize::Immediate, "Inmediato"),
    (Quantize::ThirtySecond, "1/32"), (Quantize::Sixteenth, "1/16"), (Quantize::Eighth, "1/8"),
    (Quantize::Quarter, "1/4"), (Quantize::Half, "1/2"),
    (Quantize::One, "1 compás"), (Quantize::Two, "2 compases"),
    (Quantize::Four, "4 compases"), (Quantize::Eight, "8 compases"),
];

/// Aclara un color interpolando hacia blanco.
/// `Color32::gamma_multiply` solo acepta 0.0..=1.0 (atenúa), así que para
/// aclarar hay que hacer lerp: mezclar con blanco en proporción `amt`.
fn lighten(c: egui::Color32, amt: f32) -> egui::Color32 {
    let a = amt.clamp(0.0, 1.0);
    let up = |v: u8| (v as f32 + (255.0 - v as f32) * a).round() as u8;
    egui::Color32::from_rgba_premultiplied(up(c.r()), up(c.g()), up(c.b()), c.a())
}

fn q_label(q: Quantize) -> &'static str {
    Q_LIST.iter().find(|(x, _)| *x == q).map(|(_, l)| *l).unwrap_or("1 compás")
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() > n { s.chars().take(n.saturating_sub(1)).collect::<String>() + "…" }
    else { s.to_string() }
}


/// Estado de visibilidad de ventanas flotantes (estilo FL Studio/LMMS)
pub struct WindowState {
    pub clip_launch_visible: bool,
    pub groove_machine_visible: bool,
    pub inspector_visible: bool,
    pub browser_visible: bool,
    pub mixer_visible: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        WindowState {
            clip_launch_visible: true,
            groove_machine_visible: false,
            inspector_visible: true,
            browser_visible: false,
            mixer_visible: false,
        }
    }
}


#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrooveTab {
    Pad,
    Mixer,
    Step,
}

impl Default for GrooveTab {
    fn default() -> Self {
        GrooveTab::Pad
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InspectorFocus {
    None,
    Clip,
    Track,
    GroovePad,
    GrooveMixer,
    GrooveStep,
    Browser,
}

impl Default for InspectorFocus {
    fn default() -> Self {
        InspectorFocus::None
    }
}

pub struct SessionView {
    pub windows: WindowState,
    pub groove_tab: GrooveTab,
    /// Copia del estado de Groove Machine (se refresca solo si rev cambia).
    pub groove_snap: Option<lanzadera_engine::groove::GrooveSnapshot>,
    pub groove_rev: u64,
    /// Nombres de géneros (estáticos: se cachean una sola vez).
    pub groove_genres: Vec<String>,
    /// Frase para generar patrón (ERASE).
    pub groove_phrase: String,
    /// Compás visible en el step grid.
    pub groove_bar: usize,
    pub groove_root_input: String,
    /// 0 = Piano Roll (todas las piezas) · 1 = Rack (una pieza, celdas grandes)
    pub groove_view: u8,
    /// Mostrar también las piezas sin golpes.
    pub groove_show_all: bool,
    /// Arrastre de velocidad: (pieza, paso, vel_inicial, existía).
    pub groove_drag: Option<(usize, usize, i32, bool)>,
    /// Página visible en Piano Roll (2 compases por página).
    pub groove_page: usize,
    /// Seguir automáticamente el playhead durante la reproducción.
    pub groove_follow_playhead: bool,
    /// Espejo del modo EVENT del MK3.
    pub groove_event: bool,
    /// Banco del rack visible: 0=A (1-16) · 1=B (17-32).
    pub groove_bank: usize,
    /// Nombre en edición del slot activo.
    pub groove_pat_name: String,
    pub groove_sel_pad: usize,
    /// Modo CHORDS (Library): pads = dado de muestras + locks de parámetro.
    pub groove_chords: bool,
    /// Nombre en edición para guardar/exportar presets del Groove.
    pub groove_preset_name: String,
    /// Preset elegido en el combo de la ventana Groove Machine.
    pub groove_preset_selected: String,
    pub groove_slot_clear_confirm: Option<usize>,
    pub groove_bank_name: String,
    pub groove_bank_dir: String,
    pub groove_bank_filter: String,
    pub groove_bank_files: Vec<std::path::PathBuf>,
    pub groove_bank_selected: Option<std::path::PathBuf>,
    pub groove_bank_import_pending: Option<std::path::PathBuf>,
    pub groove_pad_gain: [f32; 25],
    pub groove_pad_pan: [f32; 25],
    pub groove_pad_mute: [bool; 25],
    pub inspector_focus: InspectorFocus,
    pub browser_path: std::path::PathBuf,
    pub browser_filter: String,
    pub browser_favorites_only: bool,
    pub msg: String,
    pub project_path: String,
    pub audio_modal: Option<(usize, usize)>,
    pub audio_path_input: String,
    pub roll: PianoRoll,
    pub plugin_path_input: String,
    pub browser_open: bool,
    pub browser_list: Option<Vec<(String, String, String)>>,
    pub groove_sequencer_open: bool,
}

impl Default for SessionView {
    fn default() -> Self {
        SessionView {
            windows: WindowState::default(),
            groove_tab: GrooveTab::default(),
            groove_snap: None,
            groove_rev: 0,
            groove_genres: Vec::new(),
            groove_phrase: String::new(),
            groove_bar: 0,
            groove_root_input: String::new(),
            groove_view: 0,
            groove_show_all: true,
            groove_drag: None,
            groove_page: 0,
            groove_follow_playhead: true,
            groove_event: false,
            groove_bank: 0,
            groove_pat_name: String::new(),
            groove_sel_pad: 0,
            groove_chords: false,
            groove_preset_name: String::new(),
            groove_preset_selected: String::new(),
            groove_slot_clear_confirm: None,
            groove_bank_name: String::new(),
            groove_bank_dir: std::env::current_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."))
                .to_string_lossy()
                .into_owned(),
            groove_bank_filter: String::new(),
            groove_bank_files: Vec::new(),
            groove_bank_selected: None,
            groove_bank_import_pending: None,
            groove_pad_gain: [1.0; 25],
            groove_pad_pan: [0.0; 25],
            groove_pad_mute: [false; 25],
            inspector_focus: InspectorFocus::default(),
            browser_path: std::path::PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into())),
            browser_filter: String::new(),
            browser_favorites_only: false,
            msg: String::new(),
            project_path: "proyecto.lanzadera".into(),
            audio_modal: None,
            audio_path_input: String::new(),
            roll: PianoRoll::default(),
            plugin_path_input: String::new(),
            browser_open: false,
            browser_list: None,
            groove_sequencer_open: false,
        }
    }
}

impl SessionView {
    pub fn show(&mut self, ctx: &egui::Context, shared: &Arc<SessionShared>, audio_on: bool) {
        // ── Fase 1.4C.2b: peticiones del MK3 físico ──────────────────
        // Pestaña de Groove Machine pedida por PAD_MODE/STEP/KEYBOARD/CHORDS
        let mode_req = shared.mk3_mode_req.swap(255, std::sync::atomic::Ordering::Relaxed);
        if mode_req != 255 {
            // CHORDS (3) reutiliza la pestaña Pad: los pads siguen siendo
            // las celdas del kit, pero cambian de función (dado + locks).
            self.groove_chords = mode_req == 3;
            self.groove_tab = match mode_req {
                1 => GrooveTab::Step,
                2 => GrooveTab::Mixer,
                _ => GrooveTab::Pad,
            };
            self.windows.groove_machine_visible = true;
        }

        // Toggle de ventana pedido por MASCHINE/GROUP/AUTO/LOCK/NOTE REPEAT
        let win_req = shared.mk3_win_req.swap(255, std::sync::atomic::Ordering::Relaxed);
        if win_req != 255 {
            match win_req {
                0 => self.windows.clip_launch_visible = !self.windows.clip_launch_visible,
                1 => {
                    let v = !self.windows.groove_machine_visible;
                    self.windows.groove_machine_visible = v;
                    // Igual que el botón de la GUI: abrir arma, ocultar no apaga.
                    if v {
                        shared.caja_sched.enabled.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    shared.groove_machine_visible.store(v, std::sync::atomic::Ordering::Relaxed);
                }
                2 => self.windows.inspector_visible = !self.windows.inspector_visible,
                3 => self.windows.browser_visible = !self.windows.browser_visible,
                4 => self.windows.mixer_visible = !self.windows.mixer_visible,
                _ => {}
            }
        }

        // ── 4B.2: refrescar el snapshot de Groove Machine solo si cambió ──
        // ── 1.4C.3: celda seleccionada y compás pedidos por el MK3 ──
        let hw_pad = shared.groove_sel_pad.load(std::sync::atomic::Ordering::Relaxed) as usize;
        if hw_pad != self.groove_sel_pad { self.groove_sel_pad = hw_pad.min(24); }
        // Modo EVENT (los pads del MK3 son pasos) → vista de 1 compás
        self.groove_event = shared.mk3_event_mode.load(std::sync::atomic::Ordering::Relaxed);
        self.groove_view = shared.groove_view.load(std::sync::atomic::Ordering::Relaxed);
        self.groove_bar = shared.groove_bar.load(std::sync::atomic::Ordering::Relaxed);
        self.groove_page = shared.groove_page.load(std::sync::atomic::Ordering::Relaxed);
        self.groove_follow_playhead = shared.mk3_follow_playhead.load(std::sync::atomic::Ordering::Relaxed);
        shared.groove_follow_playhead.store(
            self.groove_follow_playhead,
            std::sync::atomic::Ordering::Relaxed,
        );
        shared.mk3_follow_playhead.store(
            self.groove_follow_playhead,
            std::sync::atomic::Ordering::Relaxed,
        );
        shared.groove_view.store(self.groove_view as u8, std::sync::atomic::Ordering::Relaxed);
        shared.groove_bar.store(self.groove_bar, std::sync::atomic::Ordering::Relaxed);
        shared.groove_page.store(self.groove_page, std::sync::atomic::Ordering::Relaxed);

        let bar_req = shared.mk3_bar_req.swap(255, std::sync::atomic::Ordering::Relaxed);
        if bar_req != 255 {
            let bars = self.groove_snap.as_ref().map(|s| s.pattern.bars).unwrap_or(1).max(1);
            if bar_req == 3 {
                // SHIFT + PLAY: ir al compás que indica el playhead.
                let abs_step = shared.caja_sched.current_abs_step.load(
                    std::sync::atomic::Ordering::Relaxed,
                );
                if abs_step != u32::MAX {
                    let rack_like = self.groove_view == 1 || self.groove_event;
                    let per_page = if rack_like { 1usize } else { 2usize };
                    let pages = bars.div_ceil(per_page).max(1);
                    let abs_bar = (abs_step as usize / caja_core::pattern::STEPS_PER_BAR)
                        .min(bars.saturating_sub(1));

                    if rack_like {
                        self.groove_bar = abs_bar;
                    } else {
                        self.groove_page = (abs_bar / per_page)
                            .min(pages.saturating_sub(1));
                    }
                }
            } else if bar_req == 2 {
                // SHIFT + ◀▶ : alternar Piano Roll ↔ Rack
                self.groove_view = if self.groove_view == 0 { 1 } else { 0 };
            } else {
                let d: i32 = if bar_req == 0 { -1 } else { 1 };
                if self.groove_view == 1 || self.groove_event {
                    // Rack / EVENT: compás a compás
                    self.groove_bar = (self.groove_bar as i32 + d).clamp(0, bars as i32 - 1) as usize;
                } else {
                    // Piano Roll: de par de compases en par
                    let per = 2usize;
                    let pages = bars.div_ceil(per).max(1);
                    self.groove_page = (self.groove_page as i32 + d).clamp(0, pages as i32 - 1) as usize;
                }
            }
        }

        // Paso pedido por un pad en modo EVENT
        // EVENT: ciclo vacío → golpe(100) → suave(60) → ghost(35) → vacío
        let step_req = shared.mk3_step_req.swap(255, std::sync::atomic::Ordering::Relaxed);
        if step_req != 255 {
            let pc = caja_core::pieces::PAD_ORDER[self.groove_sel_pad.min(24)];
            let st = self.groove_bar * 16 + step_req as usize;
            let cur = self.groove_snap.as_ref().map(|sn| sn.pattern.cell[pc][st]);
            if let Some(c) = cur {
                if !c.on() {
                    // Vacío → crear. set_cell_vel NO crea celdas: hay que
                    // usar toggle_cell primero (igual que el editor de Caja).
                    shared.groove.toggle_cell(pc, st);
                    shared.groove.set_cell_vel(pc, st, 100);
                } else if c.ghost {
                    // Ghost → borrar (limpiar el flag antes de apagar)
                    shared.groove.toggle_ghost(pc, st);
                    shared.groove.toggle_cell(pc, st);
                } else if c.vel > 80 {
                    shared.groove.set_cell_vel(pc, st, 60);
                } else {
                    shared.groove.set_cell_vel(pc, st, 35);
                    shared.groove.toggle_ghost(pc, st);
                }
            }
        }

        // EVENT: pad mantenido + ENCODER → velocidad fina
        let vd = shared.mk3_vel_req.swap(0, std::sync::atomic::Ordering::Relaxed);
        if vd != 0 {
            let held = shared.mk3_held_pad.load(std::sync::atomic::Ordering::Relaxed);
            if held != 255 {
                let pc = caja_core::pieces::PAD_ORDER[self.groove_sel_pad.min(24)];
                let st = self.groove_bar * 16 + held as usize;
                if let Some(sn) = &self.groove_snap {
                    let c = sn.pattern.cell[pc][st];
                    if !c.on() {
                        shared.groove.toggle_cell(pc, st);
                        shared.groove.set_cell_vel(pc, st, (100 + vd as i32).clamp(1, 127));
                    } else {
                        shared.groove.set_cell_vel(pc, st, (i32::from(c.vel) + vd as i32).clamp(1, 127));
                    }
                }
            }
        }
        // La frase del Inspector se comparte con el botón ERASE del MK3
        {
            let mut ph = shared.groove_phrase.lock().unwrap_or_else(|e| e.into_inner());
            if *ph != self.groove_phrase { *ph = self.groove_phrase.clone(); }
        }

        let grev = shared.groove.rev();
        if grev != self.groove_rev || self.groove_snap.is_none() {
            self.groove_snap = Some(shared.groove.snapshot());
            self.groove_rev = grev;
            if self.groove_genres.is_empty() {
                if let Some(s) = &self.groove_snap {
                    self.groove_genres = s.genre_names.clone();
                }
            }
        }

        self.top_bar(ctx, shared, audio_on);

        // Publicar visibilidad de ventanas para los LEDs del MK3 (GROUP/AUTO/LOCK/NOTE REPEAT)
        let wf = (self.windows.clip_launch_visible as u8)
            | ((self.windows.groove_machine_visible as u8) << 1)
            | ((self.windows.inspector_visible as u8) << 2)
            | ((self.windows.browser_visible as u8) << 3)
            | ((self.windows.mixer_visible as u8) << 4);
        shared.window_flags.store(wf, std::sync::atomic::Ordering::Relaxed);

        // ── Máscaras exactas para los LEDs del MK3 ──
        let sel_piece = caja_core::pieces::PAD_ORDER[self.groove_sel_pad.min(24)];
        shared.groove_sel_piece.store(sel_piece as u8, std::sync::atomic::Ordering::Relaxed);
        let head = groove_head(shared);
        shared.gm_playhead.store(head, std::sync::atomic::Ordering::Relaxed);
        if let Some(sn) = &self.groove_snap {
            let mut bits: u32 = 0;
            for i in 0..32 { if sn.filled[i] { bits |= 1 << i; } }
            shared.groove_rack_bits.store(bits, std::sync::atomic::Ordering::Relaxed);
            shared.groove_active_pat.store(sn.active.min(31) as u8, std::sync::atomic::Ordering::Relaxed);
            // EVENT: los pads representan los pasos de la pieza seleccionada.
            // Así el LED refleja tanto sus ediciones como el cambio de pieza
            // realizado con el encoder, igual que la nota seleccionada en Bitwig.
            let base = self.groove_bar * 16;
            let mut sb: u32 = 0;
            let mut soft_bits: u32 = 0;
            for i in 0..16 {
                let st = base + i;
                if st < sn.pattern.steps() {
                    let cell = sn.pattern.cell[sel_piece][st];
                    if cell.on() { sb |= 1 << i; }
                    if cell.ghost {
                        sb |= 1 << (i + 16);
                    } else if cell.on() && cell.vel <= 80 {
                        // La velocidad 60 del ciclo EVENT es expresión suave;
                        // las ghost se distinguen con su propia máscara.
                        soft_bits |= 1 << i;
                    }
                }
            }
            shared.gm_step_bits.store(sb, std::sync::atomic::Ordering::Relaxed);
            shared.gm_step_soft_bits.store(
                soft_bits,
                std::sync::atomic::Ordering::Relaxed,
            );
            // Piezas que suenan en el paso del playhead (ritmo en modo PAD)
            let mut hb: u32 = 0;
            if head != 255 {
                let gstep = head as usize;
                let st = base + (gstep % 16);   // paso dentro del compás visible
                if st < sn.pattern.steps() {
                    for pad in 0..16 {
                        let pc = caja_core::pieces::PAD_ORDER[pad];
                        if sn.pattern.cell[pc][st].on() { hb |= 1 << pad; }
                    }
                }
            }
            shared.gm_hit_bits.store(hb, std::sync::atomic::Ordering::Relaxed);
        }
        shared.groove_sel_pad.store(self.groove_sel_pad.min(24) as u8, std::sync::atomic::Ordering::Relaxed);
        // Pieza real (no pad) y estado del rack, para los LEDs del MK3
        let sel_piece = caja_core::pieces::PAD_ORDER[self.groove_sel_pad.min(24)];
        shared.groove_sel_piece.store(sel_piece as u8, std::sync::atomic::Ordering::Relaxed);
        if let Some(snap) = &self.groove_snap {
            let mut bits: u32 = 0;
            for i in 0..32 { if snap.filled[i] { bits |= 1 << i; } }
            shared.groove_rack_bits.store(bits, std::sync::atomic::Ordering::Relaxed);
            shared.groove_active_pat.store(snap.active.min(31) as u8, std::sync::atomic::Ordering::Relaxed);
        }

        // Fondo oscuro global estilo FL Studio / LMMS / Lemon Studio.
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Color32::from_rgb(10, 10, 12)))
            .show(ctx, |_ui| {});

        // Clip Launch: grid + mixer + piano roll.
        if self.windows.clip_launch_visible {
            self.roll.show(ctx, shared);
            self.clip_launch_window(ctx, shared, audio_on);
        }

        // Groove Machine: Pad / Mixer / Step.
        if self.windows.groove_machine_visible {
            self.groove_machine_window(ctx, shared);
        }

        // Sub-modo publicado para el MK3 y los LEDs: 0=Pad 1=Step 2=Mixer
        // 3=Chords. Se escribe DESPUÉS de la ventana para que CHORDS gane.
        // El sub-modo publicado es SIEMPRE el de la pestaña (0/1/2), igual
        // que el que escribe la ventana: el valor ya no oscila dentro del
        // frame. CHORDS viaja en su propio flag estable.
        let sub_mode = match self.groove_tab {
            GrooveTab::Pad => 0,
            GrooveTab::Step => 1,
            GrooveTab::Mixer => 2,
        };
        shared.groove_sub_mode.store(sub_mode, std::sync::atomic::Ordering::Relaxed);
        shared.mk3_chords_mode.store(self.groove_chords, std::sync::atomic::Ordering::Relaxed);

        // Browser global.
        if self.windows.browser_visible {
            self.global_browser_window(ctx, shared);
            self.browser_window(ctx, shared);
        }

        // Mixer como ventana independiente.
        if self.windows.mixer_visible {
            self.mixer_window(ctx, shared, audio_on);
        }

        // Inspector global contextual.
        if self.windows.inspector_visible {
            self.global_inspector_window(ctx, shared);
        }

        self.audio_modal_window(ctx, shared);
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }

    // ── TOP BAR ────────────────────────────────────────────────────────────
    fn top_bar(&mut self, ctx: &egui::Context, shared: &SessionShared, audio_on: bool) {
        let mut cmds: Vec<ControlCmd> = Vec::new();
        let mut new_q: Option<Quantize> = None;
        let mut save = false;
        let mut load = false;
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading("lanzadera");
                ui.separator();
                
                // ── Botones de visibilidad de ventanas (estilo FL Studio/LMMS) ──
                let vis_btn = |ui: &mut egui::Ui, label: &str, visible: bool| -> bool {
                    let fill = if visible { T.clip_playing.gamma_multiply(0.4) } else { T.panel };
                    ui.add(egui::Button::new(label).fill(fill)).clicked()
                };
                
                if vis_btn(ui, "🎹 Clip Launch", self.windows.clip_launch_visible) {
                    self.windows.clip_launch_visible = !self.windows.clip_launch_visible;
                }
                let groove_on = shared.caja_sched.enabled.load(std::sync::atomic::Ordering::Relaxed);
                let groove_step = groove_head(shared);
                let groove_label = if groove_on && groove_step != 255 {
                    format!("🥁 GM {}/16", groove_step + 1)
                } else if groove_on {
                    "🥁 GM ON".to_string()
                } else {
                    "🥁 Groove Machine".to_string()
                };
                if vis_btn(ui, &groove_label, self.windows.groove_machine_visible) {
                    let v = !self.windows.groove_machine_visible;
                    self.windows.groove_machine_visible = v;
                    // GM gobierna SOLO ventana + integración MK3. Abrir arma la
                    // batería; ocultar NO la apaga: el patrón sigue hasta Stop.
                    // Separación de paneles vs transporte, como en el session
                    // view de MAGDA (GPL-3) — concepto, no código.
                    if v {
                        shared.caja_sched.enabled.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    shared.groove_machine_visible.store(v, std::sync::atomic::Ordering::Relaxed);
                }
                // Armado independiente de la batería: silenciar sin detener el
                // transporte ni cerrar la ventana.
                if vis_btn(ui, "🥁", groove_on) {
                    shared.caja_sched.enabled.store(!groove_on, std::sync::atomic::Ordering::Relaxed);
                }
                if vis_btn(ui, "🎲 Chords", self.groove_chords) {
                    self.groove_chords = !self.groove_chords;
                    if self.groove_chords {
                        self.windows.groove_machine_visible = true;
                        shared.caja_sched.enabled.store(true, std::sync::atomic::Ordering::Relaxed);
                        shared.groove_machine_visible.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                if vis_btn(ui, "🔍 Inspector", self.windows.inspector_visible) {
                    self.windows.inspector_visible = !self.windows.inspector_visible;
                }
                if vis_btn(ui, "📁 Browser", self.windows.browser_visible) {
                    self.windows.browser_visible = !self.windows.browser_visible;
                }
                if vis_btn(ui, "🎚 Mixer", self.windows.mixer_visible) {
                    self.windows.mixer_visible = !self.windows.mixer_visible;
                }
                ui.separator();
                
                let (mut tempo, playing, recording, q) = {
                    let st = shared.state.lock().unwrap();
                    (st.tempo, st.playing, st.recording, st.quantize)
                };
                ui.label("BPM");
                if ui.add(egui::DragValue::new(&mut tempo).speed(0.1).range(20.0..=666.0)).changed() {
                    cmds.push(ControlCmd::SetTempo { bpm: tempo as f32 });
                }
                if ui.add(egui::Button::new(if playing { "⏸ Pause" } else { "▶ Play" })
                    .fill(if playing { T.clip_playing.gamma_multiply(0.5) } else { T.panel })).clicked() {
                    // Segundo press = pausa: el transporte se congela y al volver
                    // a Play retoma en el mismo compás (no resetea el reloj).
                    cmds.push(if playing { ControlCmd::Stop } else { ControlCmd::Play });
                }
                if ui.add(egui::Button::new("⏺ Rec")
                    .fill(if recording { T.clip_rec.gamma_multiply(0.6) } else { T.panel })).clicked() {
                    cmds.push(ControlCmd::Record);
                }
                if ui.button("⏹ Todo").clicked() {
                    // Stop global único: para todo y vuelve al compás 1.1.
                    lanzadera_engine::audio_io::STOP_ALL_REQ.store(true, std::sync::atomic::Ordering::Relaxed);
                    cmds.push(ControlCmd::StopAll);
                }
                ui.separator();
                ui.separator();
                ui.label("Quantize");
                let mut q_mut = q;
                egui::ComboBox::from_id_source("quantize_global")
                    .selected_text(q_label(q)).show_ui(ui, |ui| {
                        for (qq, lbl) in Q_LIST {
                            ui.selectable_value(&mut q_mut, qq, lbl);
                        }
                    });
                if q_mut != q { new_q = Some(q_mut); }
                ui.separator();
                ui.label(if audio_on { "🔊 audio" } else { "🔇 sin audio" });
                let status = match &*shared.status.lock().unwrap() {
                    LoadStatus::Idle => String::new(),
                    LoadStatus::Loading { .. } => "Cargando audio…".into(),
                    LoadStatus::Ready => "Audio listo".into(),
                    LoadStatus::Error(e) => format!("⚠ {}", e),
                };
                if !status.is_empty() { ui.label(egui::RichText::new(status).color(T.text_dim).size(11.0)); }
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("Proyecto");
                ui.add(egui::TextEdit::singleline(&mut self.project_path).desired_width(260.0));
                if ui.button("💾 Guardar").clicked() { save = true; }
                if ui.button("📂 Cargar").clicked() { load = true; }
                if !self.msg.is_empty() {
                    ui.label(egui::RichText::new(&self.msg).color(T.accent).size(11.0));
                }
            });
        });
        for c in cmds { shared.apply_control(c); }
        if let Some(q) = new_q { shared.edit(|st| st.quantize = q); }
        if save {
            self.msg = match shared.save_to_file(&self.project_path) {
                Ok(()) => format!("Guardado {}", self.project_path), Err(e) => e,
            };
        }
        if load {
            self.msg = match shared.load_from_file(&self.project_path) {
                Ok(()) => format!("Cargado {}", self.project_path), Err(e) => e,
            };
        }
    }

    // ── CLIP GRID ──────────────────────────────────────────────────────────
    fn clip_grid(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, shared: &SessionShared, audio_on: bool) {
        let (n_tracks, n_scenes) = {
            let st = shared.state.lock().unwrap();
            (st.tracks.len(), st.scenes.len().max(1))
        };
        // ── Celdas elásticas: llenan la ventana en ancho y alto ──
        let avail = ui.available_size();
        let header_w = 120.0f32;
        let sp = 4.0f32;
        let cell_w = ((avail.x - header_w - sp * (n_scenes as f32 + 1.0))
            / n_scenes as f32).max(64.0);
        let _cell_h = ((avail.y - 28.0 - sp * (n_tracks as f32 + 1.0))
            / n_tracks as f32).max(44.0);

        let mut cmds = Vec::new();
        let mut loads = Vec::new();
        let mut deletes = Vec::new();
        let mut creates = Vec::new();
        let has_drop = ctx.input(|i| !i.raw.dropped_files.is_empty());

        egui::ScrollArea::both().id_source("clip_grid_scroll").auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("clip_grid").spacing(vec2(4.0, 4.0)).show(ui, |ui| {
                ui.label("");
                for s in 0..n_scenes {
                    let name = {
                        let st = shared.state.lock().unwrap();
                        st.scenes.get(s).map(|sc| sc.name.clone()).unwrap_or_default()
                    };
                    ui.vertical(|ui| {
                        ui.set_width(cell_w);
                        if ui.add(egui::Button::new(format!("▶ {}", trunc(&name, 9))).small()).clicked() {
                            cmds.push(ControlCmd::LaunchScene { scene: s as u8 });
                        }
                    });
                }
                ui.end_row();
                for t in 0..n_tracks {
                    let (name, color, mute, solo) = {
                        let st = shared.state.lock().unwrap();
                        let tr = &st.tracks[t];
                        (tr.name.clone(), tr.color, tr.mute, tr.solo)
                    };
                    ui.vertical(|ui| {
                        ui.set_width(header_w);
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                            ui.painter().circle_filled(r.center(), 5.0, Color32::from_rgb(color[0], color[1], color[2]));
                            // Clic en el nombre = foco de la pista. Antes era un
                            // label pasivo y solo la celda de clip daba foco.
                            let head = ui.add(
                                egui::Label::new(egui::RichText::new(trunc(&name, 10)).size(12.0))
                                    .sense(egui::Sense::click()),
                            );
                            if head.clicked() {
                                let cur = shared.state.lock().unwrap().selected_track;
                                cmds.push(ControlCmd::SelectTrack {
                                    delta: (t as i32 - cur as i32) as i8,
                                });
                            }
                        });
                        ui.horizontal(|ui| {
                            if ui.add(egui::Button::new("M").small()
                                .fill(if mute { T.clip_stopping.gamma_multiply(0.7) } else { T.panel })).clicked() {
                                cmds.push(ControlCmd::ToggleMute { track: t as u8 });
                            }
                            if ui.add(egui::Button::new("S").small()
                                .fill(if solo { T.clip_triggered.gamma_multiply(0.6) } else { T.panel })).clicked() {
                                cmds.push(ControlCmd::ToggleSolo { track: t as u8 });
                            }
                        });
                    });
                    for s in 0..n_scenes {
                        self.clip_cell(ui, ctx, shared, t, s, audio_on, has_drop,
                                       &mut cmds, &mut loads, &mut deletes, &mut creates);
                    }
                    ui.end_row();
                }
            });
        });

        for c in cmds { shared.apply_control(c); }
        for (t, s) in creates {
            shared.edit(|st| {
                let mut creado = false;
                if let Some(tr) = st.tracks.get_mut(t) {
                    if let Some(slot) = tr.clips.get_mut(s) {
                        // Defaults para directo: 4 compases y lanzamiento
                        // inmediato. Antes: 1 compás + quantize 1 compás, que
                        // obligaba a editar el clip a mano antes de grabar.
                        // Defaults para directo: 8 compases y quantize global
                        // (no Immediate, para que el lanzamiento espere frontera).
                        *slot = Some(Clip {
                            name: format!("Clip {}", s + 1),
                            loop_bars: 8,
                            quantize: st.quantize,
                            ..Default::default()
                        });
                        creado = true;
                    }
                }
                // El clip recién creado toma el foco del Inspector, igual que
                // CreateClip desde el KeyLab. Antes esta ruta escribía el slot
                // sin mover la selección y el Inspector seguía en el vacío.
                if creado {
                    st.selected_track = t;
                    st.selected_scene = s;
                    st.selected_clip = Some((t, s));
                }
            });
        }
        for (t, s) in deletes {
            // ¿Estaba sonando/grabando? Entonces hay que matar la voz del motor,
            // no solo borrar el slot: voices[t].midi sostiene un Arc que sigue
            // emitiendo notas aunque el clip ya no exista en el estado.
            let was_active = {
                let st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                st.tracks.get(t)
                    .and_then(|tr| tr.clips.get(s))
                    .and_then(|c| c.as_ref())
                    .map(|c| matches!(c.state,
                        ClipState::Playing | ClipState::Triggered
                        | ClipState::Recording | ClipState::Overdubbing
                        | ClipState::Stopping | ClipState::RecQueued))
                    .unwrap_or(false)
            };
            shared.edit(|st| {
                if let Some(tr) = st.tracks.get_mut(t) {
                    if let Some(slot) = tr.clips.get_mut(s) { *slot = None; }
                }
                if st.selected_clip == Some((t, s)) { st.selected_clip = None; }
            });
            shared.samples.clear(t, s);
            // Faltaba esto: el NoteStore conservaba el Arc del clip borrado.
            shared.notes.clear(t, s);
            shared.rt.set_clip_state(t, s, ClipState::Empty);
            if was_active {
                if let Ok(mut q) = lanzadera_engine::audio_io::KILL_TRACKS.lock() {
                    q.push(t);
                }
            }
        }
        for (t, s, p) in loads { shared.request_load(t, s, p); }
    }

    #[allow(clippy::too_many_arguments)]
    fn clip_cell(
        &mut self, ui: &mut egui::Ui, ctx: &egui::Context, shared: &SessionShared,
        t: usize, s: usize, audio_on: bool, has_drop: bool,
        cmds: &mut Vec<ControlCmd>, loads: &mut Vec<(usize, usize, String)>,
        deletes: &mut Vec<(usize, usize)>, creates: &mut Vec<(usize, usize)>,
    ) {
        let clip = {
            shared.state.lock().unwrap().tracks.get(t)
                .and_then(|tr| tr.clips.get(s)).cloned().flatten()
        };
        let rt_state = if audio_on { shared.rt.clip_state(t, s) }
            else { clip.as_ref().map(|c| c.state).unwrap_or(ClipState::Empty) };
        let selected = { shared.state.lock().unwrap().selected_clip == Some((t, s)) };

        let (rect, resp) = ui.allocate_exact_size(vec2(96.0, 54.0), Sense::click_and_drag());
        let p = ui.painter_at(rect);
        let fill = match &clip {
            Some(c) => Color32::from_rgb(c.color[0], c.color[1], c.color[2]).gamma_multiply(0.5),
            None => T.clip_empty,
        };
        p.rect_filled(rect, Rounding::same(4.0), fill);

        // Waveform simplificada (picos pre-calculados por el loader).
        if let Some(c) = &clip {
            if c.audio_path.is_some() {
                if let Some(sd) = shared.samples.load(t, s) {
                    let n = sd.peaks.len().max(1);
                    let w = rect.width() - 8.0;
                    let mid_y = rect.bottom() - 14.0;
                    for (i, pk) in sd.peaks.iter().enumerate() {
                        let x = rect.left() + 4.0 + w * i as f32 / n as f32;
                        let h = (pk.clamp(0.0, 1.0) * 10.0).max(0.5);
                        p.line_segment(
                            [pos2(x, mid_y - h), pos2(x, mid_y + h)],
                            Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 70)),
                        );
                    }
                }
            }
            p.text(rect.left_center() + vec2(6.0, -10.0), Align2::LEFT_CENTER,
                   trunc(&c.name, 12), FontId::proportional(11.0), Color32::WHITE);
        }

        // Estado animado (borde pulsante si Playing, como pide el spec).
        let time = ctx.input(|i| i.time) as f32;
        match rt_state {
            ClipState::Playing => {
                let pulse = 0.5 + 0.5 * (time * 5.0).sin();
                p.rect_stroke(rect, Rounding::same(4.0), Stroke::new(1.5 + 1.5 * pulse, T.clip_playing));
                p.text(rect.right_top() + vec2(-5.0, 3.0), Align2::RIGHT_TOP, "▶",
                       FontId::proportional(10.0), T.clip_playing);
                if shared.rt.active_scene(t) == Some(s as u8) {
                    if let Some(c) = &clip {
                        let loop_ticks = c.loop_bars.max(1) as u32 * TICKS_PER_BAR as u32;
                        let frac = (shared.rt.playhead(t) as f32 / loop_ticks as f32).clamp(0.0, 1.0);
                        p.rect_filled(
                            Rect::from_min_size(rect.left_bottom() + vec2(2.0, -4.0),
                                                vec2((rect.width() - 4.0) * frac, 2.0)),
                            Rounding::same(1.0), T.clip_playing,
                        );
                    }
                }
            }
            ClipState::Triggered => {
                let blink = ((time * 6.0).sin() > 0.0) as u8 as f32;
                p.rect_stroke(rect, Rounding::same(4.0), Stroke::new(2.0, T.clip_triggered.gamma_multiply(0.4 + 0.6 * blink)));
            }
            ClipState::Stopping => {
                p.rect_stroke(rect, Rounding::same(4.0), Stroke::new(2.0, T.clip_stopping));
            }
            ClipState::RecQueued => {
                // Count-in visual: el punto rojo parpadea para indicar que está
                // esperando la frontera de quantize antes de empezar a grabar.
                // Inspirado en Bitwig Studio.
                let blink = ((time * 4.0).sin() > 0.0) as u8 as f32;
                p.rect_stroke(rect, Rounding::same(4.0), Stroke::new(2.0, T.clip_rec.gamma_multiply(0.7)));
                p.circle_filled(rect.right_top() + vec2(-8.0, 8.0), 4.0, T.clip_rec.gamma_multiply(0.4 + 0.6 * blink));
            }
            ClipState::Recording | ClipState::Overdubbing => {
                p.rect_stroke(rect, Rounding::same(4.0), Stroke::new(2.0, T.clip_rec));
                p.circle_filled(rect.right_top() + vec2(-8.0, 8.0), 4.0, T.clip_rec);
            }
            ClipState::Empty => {}
        }
        if selected {
            p.rect_stroke(rect.shrink(1.5), Rounding::same(4.0), Stroke::new(1.0, Color32::from_gray(220)));
        }

        // Interacción: click lanza (o selecciona si vacío), derecho = menú.
        // IMPORTANTE: un double_click también genera clicked() en egui.
        // Si lanzamos en el single-click del doble-click, el clip se
        // reinicia desde 1.1 y se superpone al audio que ya estaba
        // sonando (bug reportado en vivo). Por eso el launch solo ocurre
        // en single-click real.
        if resp.double_clicked() && clip.is_some() {
            // Doble click → abre el PianoRoll del clip (spec §9.1).
            // No lanza ni reinicia el clip: solo edita.
            self.roll.open_for(t, s);
            cmds.push(ControlCmd::SelectClip { track: t as u8, scene: s as u8 });
            self.inspector_focus = InspectorFocus::Clip;
        } else if resp.clicked() {
            if clip.is_some() {
                // Clic izquierdo: el slot SIEMPRE toma el foco del Inspector.
                // Si tiene contenido, además se lanza. SelectClip va después
                // para que el foco quede aquí aunque launch_clip salga antes
                // (modo Toggle sobre un clip que ya suena).
                cmds.push(ControlCmd::LaunchClip { track: t as u8, scene: s as u8 });
                cmds.push(ControlCmd::SelectClip { track: t as u8, scene: s as u8 });
                self.inspector_focus = InspectorFocus::Clip;
            } else {
                // Flujo Bitwig: pista armada + celda vacía + click = grabar con
                // count-in. La celda se pone roja parpadeante (RecQueued) y la
                // captura abre en el 1.1 real, con margen para la latencia MIDI.
                // Sin arm, el click solo selecciona (comportamiento anterior).
                let armed = { shared.state.lock().unwrap().tracks.get(t).map(|tr| tr.arm).unwrap_or(false) };
                cmds.push(ControlCmd::SelectClip { track: t as u8, scene: s as u8 });
                if armed {
                    cmds.push(ControlCmd::Record);
                }
                self.inspector_focus = InspectorFocus::Track;
            }
        }
        if resp.secondary_clicked() {
            cmds.push(ControlCmd::SelectClip { track: t as u8, scene: s as u8 });
            self.inspector_focus = if clip.is_some() {
                InspectorFocus::Clip
            } else {
                InspectorFocus::Track
            };
        }
        resp.context_menu(|ui| {
            if clip.is_some() {
                if ui.button("⏹ Stop").clicked() { cmds.push(ControlCmd::StopClip { track: t as u8 }); ui.close_menu(); }
                if ui.button("🎹 Piano Roll").clicked() { self.roll.open_for(t, s); ui.close_menu(); }
            } else if ui.button("＋ Crear clip").clicked() { creates.push((t, s)); ui.close_menu(); }
            if ui.button("📂 Cargar audio…").clicked() {
                self.audio_modal = Some((t, s));
                self.audio_path_input = clip.as_ref().and_then(|c| c.audio_path.clone()).unwrap_or_default();
                ui.close_menu();
            }
            if clip.is_some() && ui.button("🗑 Borrar").clicked() { deletes.push((t, s)); ui.close_menu(); }
        });

        // Drag & drop de archivos sobre la celda.
        if has_drop {
            if let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) {
                if rect.contains(pos) {
                    let path = ctx.input(|i| i.raw.dropped_files.first().and_then(|f| f.path.clone()));
                    if let Some(pb) = path { loads.push((t, s, pb.to_string_lossy().into_owned())); }
                    p.rect_stroke(rect, Rounding::same(4.0), Stroke::new(2.0, T.accent));
                }
            }
        }
    }

    // ── MIXER (strips estilo Vibez) ────────────────────────────────────────
    fn mixer(&mut self, ui: &mut egui::Ui, shared: &SessionShared, audio_on: bool) {
        let n = { shared.state.lock().unwrap().tracks.len() };
        let mut cmds = Vec::new();
        egui::ScrollArea::horizontal().id_source("mixer_scroll").show(ui, |ui| {
            ui.horizontal_top(|ui| {
                for t in 0..n { self.strip(ui, shared, Some(t), audio_on, &mut cmds); }
                self.strip(ui, shared, None, audio_on, &mut cmds);
            });
        });
        for c in cmds { shared.apply_control(c); }
    }

    fn strip(&mut self, ui: &mut egui::Ui, shared: &SessionShared, track: Option<usize>,
             audio_on: bool, cmds: &mut Vec<ControlCmd>) {
        let (name, mut vol, mut pan, mute, solo, arm) = match track {
            Some(t) => {
                let st = shared.state.lock().unwrap();
                let tr = &st.tracks[t];
                (tr.name.clone(), tr.volume, tr.pan, tr.mute, tr.solo, tr.arm)
            }
            None => {
                let st = shared.state.lock().unwrap();
                ("MASTER".to_string(), st.master_volume, 0.0, false, false, false)
            }
        };
        let level = if audio_on { shared.rt.peak(track.unwrap_or(MAX_TRACKS)) } else { 0.0 };
        egui::Frame::none().fill(T.strip_bg).rounding(Rounding::same(6.0)).inner_margin(6.0).show(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_width(64.0);
                // Clic en el nombre = foco de la pista (igual que en el grid).
                let head = ui.add(
                    egui::Label::new(egui::RichText::new(trunc(&name, 8)).size(11.0))
                        .sense(egui::Sense::click()),
                );
                if head.clicked() {
                    if let Some(t) = track {
                        let cur = shared.state.lock().unwrap().selected_track;
                        cmds.push(ControlCmd::SelectTrack {
                            delta: (t as i32 - cur as i32) as i8,
                        });
                    }
                }
                if let Some(t) = track {
                    if widgets::knob(ui, &mut pan, -1.0, 1.0, 0.0, "Pan") {
                        cmds.push(ControlCmd::SetPan { track: t as u8, value: pan });
                    }
                } else { ui.add_space(50.0); }
                if widgets::fader(ui, &mut vol, 110.0, level) {
                    cmds.push(match track {
                        Some(t) => ControlCmd::SetVolume { track: t as u8, value: vol },
                        None => ControlCmd::SetMasterVol { value: vol },
                    });
                }
                let db = if vol <= 0.0001 { "-inf".to_string() } else { format!("{:.1} dB", 20.0 * vol.log10()) };
                ui.label(egui::RichText::new(db).size(9.0).color(T.text_dim));
                if let Some(t) = track {
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new("M").small()
                            .fill(if mute { T.clip_stopping.gamma_multiply(0.7) } else { T.panel })).clicked() {
                            cmds.push(ControlCmd::ToggleMute { track: t as u8 });
                        }
                        if ui.add(egui::Button::new("S").small()
                            .fill(if solo { T.clip_triggered.gamma_multiply(0.6) } else { T.panel })).clicked() {
                            cmds.push(ControlCmd::ToggleSolo { track: t as u8 });
                        }
                    });
                    if ui.add(egui::Button::new("● Arm").small()
                        .fill(if arm { T.clip_rec.gamma_multiply(0.6) } else { T.panel })).clicked() {
                        cmds.push(ControlCmd::ToggleArm { track: t as u8 });
                    }
                }
            });
        });
    }

    // ── INSPECTOR (panel derecho contextual) ───────────────────────────────
    #[allow(dead_code)] // Reemplazado por global_inspector_window (Fase 1.4B)
    /// Contenido del inspector de Clip Launch: clip + EQ + plugin CLAP + parámetros.
    ///
    /// Recibe `ui` en vez de crear su propio SidePanel, para poder vivir dentro
    /// de la ventana global de inspector. Inspector sensible al contexto, como
    /// el "context-sensitive editor" de MAGDA (GPL-3) — concepto, no código.
    fn inspector_clip_body(&mut self, ui: &mut egui::Ui, shared: &SessionShared) {
        let mut clip_edit: Option<(usize, usize, Clip)> = None;
        let mut eq_edit: Option<(usize, f32, f32, f32, bool, bool, f32, f32)> = None;
        let mut open_modal = None;
        let mut open_roll = None;
        let mut plugin_load: Option<(usize, String)> = None;
        let mut plugin_clear: Option<usize> = None;
        {
            ui.heading("Inspector");
            let (sel, t) = {
                let st = shared.state.lock().unwrap();
                (st.selected_clip, st.selected_track)
            };
            if let Some((t, s)) = sel {
                let clip = { shared.state.lock().unwrap().tracks.get(t).and_then(|tr| tr.clips.get(s)).cloned().flatten() };
                if let Some(mut c) = clip {
                    ui.label(egui::RichText::new(format!("Pista {} · Escena {}", t + 1, s + 1)).size(11.0).color(T.text_dim));
                    let mut changed = false;
                    ui.horizontal(|ui| {
                        ui.label("Nombre");
                        changed |= ui.text_edit_singleline(&mut c.name).changed();
                    });
                    ui.horizontal(|ui| {
                        ui.label("Loop (compases)");
                        changed |= ui.add(egui::DragValue::new(&mut c.loop_bars).range(1..=255)).changed();
                    });
                    ui.horizontal(|ui| {
                        ui.label("Repeticiones (0=∞)");
                        changed |= ui.add(egui::DragValue::new(&mut c.loop_count).range(0..=64)).changed();
                    });
                    ui.horizontal(|ui| {
                        ui.label("Quantize");
                        egui::ComboBox::from_id_source("clip_q").selected_text(q_label(c.quantize)).show_ui(ui, |ui| {
                            for (q, lbl) in Q_LIST { changed |= ui.selectable_value(&mut c.quantize, q, lbl).changed(); }
                        });
                    });
                    ui.horizontal(|ui| {
                        ui.label("Launch");
                        egui::ComboBox::from_id_source("clip_launch").selected_text(format!("{:?}", c.launch_mode)).show_ui(ui, |ui| {
                            for m in [LaunchMode::Trigger, LaunchMode::Toggle, LaunchMode::Gate, LaunchMode::Legato, LaunchMode::Repeat] {
                                changed |= ui.selectable_value(&mut c.launch_mode, m, format!("{:?}", m)).changed();
                            }
                        });
                    });
                    ui.horizontal(|ui| {
                        ui.label("Follow");
                        // El destino elegido se conserva al reabrir el combo.
                        let jump_dest = match c.follow_action {
                            FollowAction::JumpTo(n) => n,
                            _ => 0,
                        };
                        egui::ComboBox::from_id_source("clip_follow")
                            .selected_text(follow_label(c.follow_action))
                            .show_ui(ui, |ui| {
                                for f in [FollowAction::None, FollowAction::Next, FollowAction::Previous, FollowAction::First,
                                          FollowAction::Last, FollowAction::Stop, FollowAction::Random, FollowAction::Again,
                                          FollowAction::JumpTo(jump_dest)] {
                                    changed |= ui.selectable_value(&mut c.follow_action, f, follow_label(f)).changed();
                                }
                            });
                        if let FollowAction::JumpTo(n) = c.follow_action {
                            let max = lanzadera_core::MAX_SCENES as i32 - 1;
                            let mut dest = n as i32;
                            if ui.add(egui::DragValue::new(&mut dest)
                                .range(0..=max).speed(0.2).prefix("→ escena ")).changed()
                            {
                                c.follow_action = FollowAction::JumpTo(dest.clamp(0, max) as u8);
                                changed = true;
                            }
                        }
                    });
                    changed |= ui.add(egui::Slider::new(&mut c.probability, 0.0..=1.0).text("Probabilidad")).changed();
                    ui.separator();
                    if let Some(p) = &c.audio_path {
                        ui.label(egui::RichText::new(format!("🎵 {}", trunc(p, 26))).size(10.0).color(T.text_dim));
                    }
                    if ui.button("📂 Cargar audio…").clicked() { open_modal = Some((t, s, c.audio_path.clone())); }
                    if ui.button("🎹 Piano Roll").clicked() { open_roll = Some((t, s)); }
                    if changed { clip_edit = Some((t, s, c)); }
                } else {
                    ui.label("Slot vacío — crea un clip o arrastra un WAV.");
                }
            } else {
                ui.label("Selecciona un clip (click derecho o click en slot vacío).");
            }
            ui.separator();
            ui.label(egui::RichText::new("Plugin CLAP").size(12.0));
            if !lanzadera_engine::CLAP_ENABLED {
                ui.label(egui::RichText::new("Compila con --features clap").size(10.0).color(T.text_dim));
            }
            let cur = { shared.state.lock().unwrap().tracks.get(t).and_then(|tr| tr.plugin_path.clone()) };
            if let Some(p) = &cur {
                ui.label(egui::RichText::new(format!("🔌 {}", trunc(p, 24))).size(10.0).color(T.accent));
            }
            ui.text_edit_singleline(&mut self.plugin_path_input);
            ui.horizontal(|ui| {
                if ui.button("Cargar").clicked() { plugin_load = Some((t, self.plugin_path_input.clone())); }
                if ui.button("Quitar").clicked() { plugin_clear = Some(t); }
                if lanzadera_engine::CLAP_ENABLED && ui.button("🔍 Explorar").clicked() { self.browser_open = true; }
            });
            ui.separator();
            ui.label(egui::RichText::new(format!("EQ · Pista {}", t + 1)).size(12.0));
            let (mut lo, mut mid, mut hi, mut hpf_on, mut lpf_on, mut hpf_hz, mut lpf_hz) = {
                let st = shared.state.lock().unwrap();
                let tr = &st.tracks[t.min(st.tracks.len() - 1)];
                (tr.eq_low, tr.eq_mid, tr.eq_high, tr.hpf_on, tr.lpf_on, tr.hpf_hz, tr.lpf_hz)
            };
            let mut eq_changed = false;
            // Curva arrastrable (doble click = plano).
            eq_changed |= eq_view::curve(ui, &mut lo, &mut mid, &mut hi);
            eq_changed |= ui.add(egui::Slider::new(&mut lo, -15.0..=15.0).text("Low")).changed();
            eq_changed |= ui.add(egui::Slider::new(&mut mid, -15.0..=15.0).text("Mid")).changed();
            eq_changed |= ui.add(egui::Slider::new(&mut hi, -15.0..=15.0).text("High")).changed();
            ui.horizontal(|ui| {
                eq_changed |= ui.checkbox(&mut hpf_on, "HPF").changed();
                eq_changed |= ui.add(egui::DragValue::new(&mut hpf_hz).range(20.0..=2000.0).suffix(" Hz")).changed();
            });
            ui.horizontal(|ui| {
                eq_changed |= ui.checkbox(&mut lpf_on, "LPF").changed();
                eq_changed |= ui.add(egui::DragValue::new(&mut lpf_hz).range(200.0..=20000.0).suffix(" Hz")).changed();
            });
            if eq_changed { eq_edit = Some((t, lo, mid, hi, hpf_on, lpf_on, hpf_hz, lpf_hz)); }

            // ── Parámetros del plugin CLAP (Fase 3G) ──
            let params = { shared.state.lock().unwrap().tracks.get(t).map(|tr| tr.plugin_params.clone()).unwrap_or_default() };
            if !params.is_empty() {
                ui.separator();
                ui.label(egui::RichText::new("Parámetros del plugin").size(12.0).strong());
                egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                    for (i, (_id, name, value)) in params.iter().enumerate() {
                        ui.horizontal(|ui| {
                            let mut v = *value;
                            if ui.add(egui::Slider::new(&mut v, 0.0..=1.0).show_value(false)).changed() {
                                // Actualizamos estado visual y encolamos el cambio al RT.
                                shared.set_plugin_param(t, i, v);
                                shared.queue_plugin_param(t, i, v);
                            }
                            ui.label(egui::RichText::new(trunc(name, 12)).size(10.0));
                            ui.label(egui::RichText::new(format!("{:.2}", v)).size(9.0).color(T.text_dim));
                        });
                    }
                });
            }
        }
        if let Some((t, s, c)) = clip_edit { shared.edit(|st| st.set_clip(t, s, c)); }
        if let Some((t, lo, mid, hi, hpf_on, lpf_on, hpf_hz, lpf_hz)) = eq_edit {
            shared.edit(|st| {
                if let Some(tr) = st.tracks.get_mut(t) {
                    tr.eq_low = lo; tr.eq_mid = mid; tr.eq_high = hi;
                    tr.hpf_on = hpf_on; tr.lpf_on = lpf_on; tr.hpf_hz = hpf_hz; tr.lpf_hz = lpf_hz;
                }
            });
        }
        if let Some((t, s)) = open_roll { self.roll.open_for(t, s); }
        if let Some((t, p)) = plugin_load {
            if !p.is_empty() {
                shared.set_plugin(t, Some(p), None);
                self.msg = "Reiniciando audio con el plugin…".into();
            }
        }
        if let Some(t) = plugin_clear {
            shared.set_plugin(t, None, None);
            self.msg = "Plugin quitado (sinte interno)".into();
        }
        if let Some((t, s, p)) = open_modal {
            self.audio_modal = Some((t, s));
            self.audio_path_input = p.unwrap_or_default();
        }
    }

    // ── Browser de plugins CLAP (Fase 3E) ─────────────────────────────────
    fn browser_window(&mut self, ctx: &egui::Context, shared: &SessionShared) {
        if !self.browser_open { return; }
        if self.browser_list.is_none() { self.browser_list = Some(lanzadera_engine::clap_scan()); }
        let t = { shared.state.lock().unwrap().selected_track };
        let mut open = self.browser_open;
        let mut chosen: Option<(String, String)> = None;
        egui::Window::new("Plugins CLAP").open(&mut open).default_size([460.0, 400.0])
            .collapsible(false).show(ctx, |ui| {
                ui.label(egui::RichText::new(format!("Destino: Pista {}", t + 1)).color(T.text_dim));
                if let Some(list) = &self.browser_list {
                    if list.is_empty() {
                        ui.label("Sin plugins en ~/.clap, ~/.local/lib/clap, /usr/lib/clap ni /usr/local/lib/clap");
                    } else {
                        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                            for (path, id, name) in list {
                                ui.horizontal(|ui| {
                                    if ui.button("➕").clicked() { chosen = Some((path.clone(), id.clone())); }
                                    ui.vertical(|ui| {
                                        ui.label(egui::RichText::new(name).strong());
                                        ui.label(egui::RichText::new(trunc(path, 50)).size(9.0).color(T.text_dim));
                                    });
                                });
                            }
                        });
                    }
                }
                if ui.button("⟳ Re-escanear").clicked() { self.browser_list = Some(lanzadera_engine::clap_scan()); }
            });
        self.browser_open = open;
        if let Some((path, id)) = chosen {
            shared.set_plugin(t, Some(path), Some(id));
            self.msg = "Cargando plugin (reiniciando audio)…".into();
            self.browser_open = false;
            self.browser_list = None;
        }
    }

    // ── Modal de carga de audio ────────────────────────────────────────────
    fn audio_modal_window(&mut self, ctx: &egui::Context, shared: &SessionShared) {
        let mut open = self.audio_modal.is_some();
        let mut do_load = None;
        let mut cancel = false;
        if let Some((t, s)) = self.audio_modal {
            egui::Window::new("Cargar audio en clip").open(&mut open).collapsible(false).show(ctx, |ui| {
                ui.label(format!("Pista {} · Escena {}", t + 1, s + 1));
                ui.label("Ruta WAV/FLAC/OGG (o arrastra el archivo sobre la celda):");
                ui.text_edit_singleline(&mut self.audio_path_input);
                ui.horizontal(|ui| {
                    if ui.button("Cargar").clicked() { do_load = Some((t, s, self.audio_path_input.clone())); }
                    if ui.button("Cancelar").clicked() { cancel = true; }
                });
            });
        }
        if !open || cancel { self.audio_modal = None; }
        if let Some((t, s, p)) = do_load {
            if !p.is_empty() { shared.request_load(t, s, p); }
            self.audio_modal = None;
        }
    }
}

// ── Editor de patrones de Groove Machine (Fase 4A.4) ──
impl SessionView {
    #[allow(dead_code)] // Reemplazado por groove_step_tab dentro de Groove Machine (Fase 1.4B)
    fn caja_sequencer_window(&mut self, ctx: &egui::Context, shared: &Arc<SessionShared>) {
        if !self.groove_sequencer_open { return; }
        let mut open = self.groove_sequencer_open;
        egui::Window::new("Groove Machine Step Sequencer").open(&mut open).default_size([600.0, 400.0])
            .collapsible(false).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("16 pasos × 16 pads (notas 36-51). Click para activar/desactivar.");
                    if ui.button("Limpiar").clicked() {
                        let mut sch = caja_core::schedule::Schedule::default();
                        sch.loop_ticks = 1920;
                        shared.caja_sched.set_schedule(sch);
                    }
                });
                ui.separator();
                
                // Leer el schedule actual
                let current_sch = shared.caja_sched.schedule.lock().unwrap().clone();
                let mut new_hits = current_sch.hits().to_vec();
                let mut changed = false;
                
                egui::ScrollArea::both().show(ui, |ui| {
                    egui::Grid::new("caja_seq_grid").spacing(vec2(4.0, 4.0)).show(ui, |ui| {
                        // Cabecera de pasos
                        ui.label("");
                        for step in 0..16 {
                            let is_beat = step % 4 == 0;
                            let color = if is_beat { egui::Color32::WHITE } else { T.text_dim };
                            ui.label(egui::RichText::new(format!("{}", step + 1)).color(color).size(10.0));
                        }
                        ui.end_row();
                        
                        // Filas de pads (16 pads, notas 36-51)
                        for pad in 0..16 {
                            let note = 36 + pad as u8;
                            ui.label(egui::RichText::new(format!("Pad {}", pad + 1)).size(10.0));
                            for step in 0..16 {
                                let tick = (step * 120) as u32; // 1920 / 16 = 120 ticks por paso
                                let has_hit = new_hits.iter().any(|h| h.note == note && h.step as u32 == step as u32);
                                
                                let btn = egui::Button::new(if has_hit { "■" } else { "□" })
                                    .fill(if has_hit { T.accent } else { T.panel })
                                    .stroke(Stroke::new(1.0, T.text_dim));
                                
                                if ui.add(btn).clicked() {
                                    if has_hit {
                                        new_hits.retain(|h| !(h.note == note && h.step as u32 == step as u32));
                                    } else {
                                        new_hits.push(caja_core::timing::Hit {
                                            tick: (tick as u16),
                                            piece: pad as u8,
                                            step: step as u8,
                                            note: note,
                                            vel: 100,
                                            dur: 60,
                                        });
                                    }
                                    changed = true;
                                }
                            }
                            ui.end_row();
                        }
                    });
                });
                
                if changed {
                    let mut sch = caja_core::schedule::Schedule::default();
                    sch.loop_ticks = 1920;
                    sch.fill(1920, &new_hits);
                    shared.caja_sched.set_schedule(sch);
                }
            });
        self.groove_sequencer_open = open;
    }
}


// ─────────────────────────────────────────────────────────────
// Fase 1.4A — Ventanas flotantes estilo FL Studio / LMMS
// ─────────────────────────────────────────────────────────────

impl SessionView {
    fn window_frame() -> egui::Frame {
        egui::Frame::none()
            .fill(Color32::from_rgb(18, 18, 22))
            .stroke(Stroke::new(1.0, Color32::from_gray(55)))
            .rounding(6.0)
    }

    fn clip_launch_window(&mut self, ctx: &egui::Context, shared: &Arc<SessionShared>, audio_on: bool) {
        let mut open = self.windows.clip_launch_visible;
        egui::Window::new("🎹 Clip Launch")
            .open(&mut open)
            .default_size([980.0, 560.0])
            .min_size([320.0, 200.0])
            .max_size([8000.0, 8000.0])
            .constrain(false)
            .resizable(true)
            .frame(Self::window_frame())
            .show(ctx, |ui| {
                // El mixer ya es ventana aparte: el grid usa TODO el alto.
                let sz = ui.available_size();
                ui.allocate_ui_with_layout(
                    sz, egui::Layout::top_down(egui::Align::LEFT),
                    |ui| { self.clip_grid(ui, ctx, shared, audio_on); },
                );
            });
        self.windows.clip_launch_visible = open;
    }

    fn groove_machine_window(&mut self, ctx: &egui::Context, shared: &Arc<SessionShared>) {
        let mut open = self.windows.groove_machine_visible;

        egui::Window::new("🥁 Groove Machine")
            .open(&mut open)
            .constrain(false)
            .max_size([8000.0, 8000.0])
            .default_size([880.0, 540.0])
            .min_size([520.0, 300.0])
            .resizable(true)
            .frame(Self::window_frame())
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.groove_tab, GrooveTab::Pad, "Pad");
                    ui.selectable_value(&mut self.groove_tab, GrooveTab::Mixer, "Mixer");
                    ui.selectable_value(&mut self.groove_tab, GrooveTab::Step, "Step");

                    ui.separator();

                    let enabled = shared.caja_sched.enabled.load(std::sync::atomic::Ordering::Relaxed);
                    if ui.add(
                        egui::Button::new(if enabled { "Motor ON" } else { "Motor OFF" })
                            .fill(if enabled { T.clip_playing.gamma_multiply(0.55) } else { T.panel })
                    ).clicked() {
                        shared.caja_sched.toggle();
                    }

                    let step = groove_head(shared);
                    if enabled && step != 255 {
                        ui.label(egui::RichText::new(format!("Step {}/16", step + 1)).color(T.accent).size(11.0));
                    }
                });

                ui.separator();

                // ── Presets del Groove (viven dentro del proyecto .lanzadera) ──
                ui.horizontal_wrapped(|ui| {
                    let presets = shared.list_groove_presets();
                    ui.label(egui::RichText::new("Preset").color(T.text_dim).size(11.0));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.groove_preset_name)
                            .desired_width(130.0)
                            .hint_text("nombre…"),
                    );
                    if ui.button("💾 Guardar")
                        .on_hover_text("Guardar el groove actual como preset (mismo nombre = reemplazar)")
                        .clicked()
                    {
                        if self.groove_preset_name.trim().is_empty() {
                            self.groove_preset_name = format!("Groove {}", presets.len() + 1);
                        }
                        match shared.save_groove_preset(&self.groove_preset_name) {
                            Ok(()) => {
                                self.groove_preset_selected = self.groove_preset_name.trim().to_string();
                                self.msg = format!("Preset guardado: {}", self.groove_preset_selected);
                            }
                            Err(e) => self.msg = e,
                        }
                    }
                    egui::ComboBox::from_id_source("gm_preset_combo")
                        .selected_text(if self.groove_preset_selected.is_empty() {
                            "elegir…".to_string()
                        } else {
                            self.groove_preset_selected.clone()
                        })
                        .width(150.0)
                        .show_ui(ui, |ui| {
                            for p in &presets {
                                ui.selectable_value(&mut self.groove_preset_selected, p.clone(), p.as_str());
                            }
                        });
                    if ui.button("📂 Cargar").clicked() && !self.groove_preset_selected.is_empty() {
                        match shared.load_groove_preset(&self.groove_preset_selected) {
                            Ok(()) => self.msg = format!("Preset cargado: {}", self.groove_preset_selected),
                            Err(e) => self.msg = e,
                        }
                    }
                    if ui.button("🗑").on_hover_text("Borrar el preset elegido").clicked()
                        && !self.groove_preset_selected.is_empty()
                    {
                        match shared.delete_groove_preset(&self.groove_preset_selected) {
                            Ok(()) => {
                                self.msg = format!("Preset borrado: {}", self.groove_preset_selected);
                                self.groove_preset_selected.clear();
                            }
                            Err(e) => self.msg = e,
                        }
                    }
                    if ui.button("⬇ Exportar")
                        .on_hover_text("Escribir <nombre>.groovepreset en la carpeta actual")
                        .clicked()
                    {
                        let n = self.groove_preset_name.trim().to_string();
                        if n.is_empty() {
                            self.msg = "Escribe un nombre para exportar.".into();
                        } else {
                            let path = format!("{}.groovepreset", n.replace('/', "_"));
                            match shared.export_groove_preset(&path) {
                                Ok(()) => self.msg = format!("Exportado {}", path),
                                Err(e) => self.msg = e,
                            }
                        }
                    }
                    if ui.button("⬆ Importar")
                        .on_hover_text("Leer <nombre>.groovepreset y guardarlo como preset")
                        .clicked()
                    {
                        let n = self.groove_preset_name.trim().to_string();
                        if n.is_empty() {
                            self.msg = "Escribe el nombre del archivo a importar (sin extensión).".into();
                        } else {
                            let path = format!("{}.groovepreset", n.replace('/', "_"));
                            match shared.import_groove_preset(&path, &n) {
                                Ok(()) => {
                                    self.groove_preset_selected = n;
                                    self.msg = format!("Importado {}", path);
                                }
                                Err(e) => self.msg = e,
                            }
                        }
                    }

                    ui.horizontal(|ui| {
                        ui.label("Banco");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.groove_bank_name)
                                .desired_width(130.0)
                                .hint_text("nombre del banco"),
                        );
                        if ui.button("⬇ Exportar banco").clicked() {
                            let bank_name = if self.groove_bank_name.trim().is_empty() {
                                "groove-bank".to_string()
                            } else {
                                self.groove_bank_name.trim().to_string()
                            };
                            self.groove_bank_name = bank_name.clone();
                            let path = format!("{}.groovebank", bank_name.replace('/', "_"));
                            match shared.export_groove_bank(&path) {
                                Ok(()) => self.msg = format!("Banco exportado: {}", path),
                                Err(e) => self.msg = e,
                            }
                        }
                    });

                ui.collapsing("Explorar bancos .groovebank", |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Carpeta");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.groove_bank_dir)
                                .desired_width(260.0),
                        );
                        if ui.button("⟳ Buscar").clicked() {
                            let dir = std::path::PathBuf::from(self.groove_bank_dir.trim());
                            match std::fs::read_dir(&dir) {
                                Ok(entries) => {
                                    let mut files: Vec<std::path::PathBuf> = entries
                                        .flatten()
                                        .map(|entry| entry.path())
                                        .filter(|path| {
                                            path.is_file()
                                                && path.extension()
                                                    .and_then(|ext| ext.to_str())
                                                    .map(|ext| ext.eq_ignore_ascii_case("groovebank"))
                                                    .unwrap_or(false)
                                        })
                                        .collect();
                                    files.sort();
                                    self.groove_bank_files = files;
                                    if self.groove_bank_selected.as_ref()
                                        .map(|selected| !self.groove_bank_files.contains(selected))
                                        .unwrap_or(false)
                                    {
                                        self.groove_bank_selected = None;
                                    }
                                    self.msg = format!(
                                        "Encontrados {} bancos",
                                        self.groove_bank_files.len()
                                    );
                                }
                                Err(e) => {
                                    self.groove_bank_files.clear();
                                    self.groove_bank_selected = None;
                                    self.msg = format!("No se pudo leer la carpeta: {}", e);
                                }
                            }
                        }
                    });

                    ui.horizontal(|ui| {
                        ui.label("Filtro");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.groove_bank_filter)
                                .desired_width(180.0)
                                .hint_text("filtrar por nombre"),
                        );
                    });

                    let favorites = shared.list_groove_bank_favorites();
                    if !favorites.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(egui::RichText::new("Favoritos").color(T.text_dim).size(10.0));
                            for favorite in &favorites {
                                let path = std::path::PathBuf::from(favorite);
                                let label = path.file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or(favorite);
                                if ui.small_button(format!("★ {}", label)).clicked() {
                                    self.groove_bank_selected = Some(path);
                                }
                            }
                        });
                    }

                    let recents = shared.list_recent_groove_banks();
                    if !recents.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(egui::RichText::new("Recientes").color(T.text_dim).size(10.0));
                            for recent in &recents {
                                let path = std::path::PathBuf::from(recent);
                                let label = path.file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or(recent);
                                if ui.small_button(format!("↩ {}", label)).clicked() {
                                    self.groove_bank_selected = Some(path);
                                }
                            }
                        });
                    }

                    let filter = self.groove_bank_filter.to_lowercase();
                    let files = self.groove_bank_files.clone();
                    egui::ScrollArea::vertical()
                        .max_height(130.0)
                        .show(ui, |ui| {
                            let mut shown = 0usize;
                            for path in files {
                                let name = path.file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("(nombre inválido)");
                                if !filter.is_empty()
                                    && !name.to_lowercase().contains(&filter)
                                {
                                    continue;
                                }
                                shown += 1;
                                let selected = self.groove_bank_selected.as_ref() == Some(&path);
                                let path_string = path.to_string_lossy().into_owned();
                                let is_favorite = favorites.iter().any(|entry| entry == &path_string);

                                ui.horizontal(|ui| {
                                    if ui.selectable_label(selected, name).clicked() {
                                        self.groove_bank_selected = Some(path.clone());
                                    }
                                    if ui.small_button(if is_favorite { "★" } else { "☆" })
                                        .on_hover_text("Alternar favorito")
                                        .clicked()
                                    {
                                        let now_favorite =
                                            shared.toggle_groove_bank_favorite(&path_string);
                                        self.msg = if now_favorite {
                                            format!("Favorito añadido: {}", name)
                                        } else {
                                            format!("Favorito quitado: {}", name)
                                        };
                                    }
                                });
                            }
                            if shown == 0 {
                                ui.label("No hay bancos que coincidan. Elige una carpeta y pulsa Buscar.");
                            }
                        });

                    if let Some(path) = self.groove_bank_selected.clone() {
                        ui.label(
                            egui::RichText::new(path.display().to_string())
                                .color(T.text_dim)
                                .size(10.0),
                        );
                        if ui.button("Preparar importación…").clicked() {
                            self.groove_bank_import_pending = Some(path);
                        }
                    }

                    if let Some(path) = self.groove_bank_import_pending.clone() {
                        ui.separator();
                        ui.colored_label(
                            egui::Color32::YELLOW,
                            format!(
                                "Esto reemplazará los presets y slots actuales: {}",
                                path.file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("banco seleccionado")
                            ),
                        );
                        ui.horizontal(|ui| {
                            if ui.button("Confirmar reemplazo").clicked() {
                                let path_text = path.to_string_lossy().into_owned();
                                match shared.import_groove_bank(&path_text) {
                                    Ok(()) => {
                                        let imported_name = {
                                            let state = shared.state
                                                .lock()
                                                .unwrap_or_else(|e| e.into_inner());
                                            state.groove_bank_name.clone()
                                        };
                                        if !imported_name.is_empty() {
                                            self.groove_bank_name = imported_name;
                                        }
                                        self.groove_bank_import_pending = None;
                                        self.msg = format!("Banco importado: {}", path_text);
                                    }
                                    Err(e) => {
                                        self.groove_bank_import_pending = None;
                                        self.msg = format!("No se importó el banco: {}", e);
                                    }
                                }
                            }
                            if ui.button("Cancelar").clicked() {
                                self.groove_bank_import_pending = None;
                            }
                        });
                    }
                });

                });
                // ── Quick slots del preset bank ──
                let (active_slot, slot_names) = {
                    let state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                    (
                        state.groove_preset_active.min(7),
                        state.groove_preset_slots.clone(),
                    )
                };
                let mut selected_slot = active_slot;

                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new("Slots rápidos").color(T.text_dim).size(11.0));

                    for slot in 0..8 {
                        let label = match &slot_names[slot] {
                            Some(name) => format!("{} · {}", slot + 1, name),
                            None => format!("{} · vacío", slot + 1),
                        };
                        if ui.selectable_label(selected_slot == slot, label).clicked() {
                            selected_slot = slot;
                        }
                    }
                });

                if selected_slot != active_slot {
                    shared.select_groove_preset_slot(selected_slot);
                    self.groove_preset_selected =
                        slot_names[selected_slot].clone().unwrap_or_default();
                    if !self.groove_preset_selected.is_empty() {
                        self.groove_preset_name = self.groove_preset_selected.clone();
                    }
                }

                let active_name = slot_names[selected_slot]
                    .as_deref()
                    .unwrap_or("Sin asignar");
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "Slot activo: {} — {}",
                            selected_slot + 1,
                            active_name
                        ))
                        .color(T.text_dim)
                        .size(11.0),
                    );

                    if ui.button("💾 Guardar en slot").clicked() {
                        let name = if self.groove_preset_name.trim().is_empty() {
                            format!("Groove {}", selected_slot + 1)
                        } else {
                            self.groove_preset_name.trim().to_string()
                        };

                        match shared.save_groove_preset_slot_named(selected_slot, &name) {
                            Ok(()) => {
                                self.groove_preset_name = name.clone();
                                self.groove_preset_selected = name.clone();
                                self.msg = format!(
                                    "Preset «{}» guardado en slot {}",
                                    name,
                                    selected_slot + 1
                                );
                            }
                            Err(e) => self.msg = e,
                        }
                    }

                    let slot_is_occupied = slot_names[selected_slot].is_some();
                    if ui.add_enabled(
                        slot_is_occupied,
                        egui::Button::new("📂 Cargar slot"),
                    ).clicked() {
                        match shared.load_groove_preset_slot(selected_slot) {
                            Ok(()) => {
                                if let Some(name) = &slot_names[selected_slot] {
                                    self.groove_preset_name = name.clone();
                                    self.groove_preset_selected = name.clone();
                                    self.msg = format!(
                                        "Preset «{}» cargado desde slot {}",
                                        name,
                                        selected_slot + 1
                                    );
                                }
                            }
                            Err(e) => self.msg = e,
                        }
                    }
                });

                ui.separator();

                // Sincronizar pestaña activa → flag atómico para el MK3
                let sub_mode_val: u8 = match self.groove_tab {
                    GrooveTab::Pad => 0,
                    GrooveTab::Mixer => 2,
                    GrooveTab::Step => 1,
                };
                shared.groove_sub_mode.store(sub_mode_val, std::sync::atomic::Ordering::Relaxed);

                match self.groove_tab {
                    GrooveTab::Pad => {
                        self.inspector_focus = InspectorFocus::GroovePad;
                        self.groove_pad_tab(ui, shared);
                    }
                    GrooveTab::Mixer => {
                        self.inspector_focus = InspectorFocus::GrooveMixer;
                        self.groove_mixer_tab(ui, shared);
                    }
                    GrooveTab::Step => {
                        self.inspector_focus = InspectorFocus::GrooveStep;
                        self.groove_step_tab(ui, shared);
                    }
                }
            });

        // Si el usuario cierra la ventana con la X, apagamos el motor.
        if !open {
            shared.caja_sched.enabled.store(false, std::sync::atomic::Ordering::Relaxed);
        }

        self.windows.groove_machine_visible = open;
    }

    fn groove_pad_tab(&mut self, ui: &mut egui::Ui, shared: &Arc<SessionShared>) {
        use caja_core::pieces::{PAD_ORDER, PIECES};

        let (active, filled, names) = match &self.groove_snap {
            Some(s) => (s.active, s.filled, s.names.clone()),
            None => (0, [false; 32], std::array::from_fn::<String, 32, _>(|_| String::new())),
        };
        let pattern_mode = shared.mk3_pattern_mode.load(std::sync::atomic::Ordering::Relaxed);

        ui.horizontal_top(|ui| {
            // ── Columna izquierda: 16 pads virtuales ──
            ui.vertical(|ui| {
                ui.set_min_width(360.0);
                ui.label(egui::RichText::new(if pattern_mode { "PADS = PATRONES (MK3)" } else { "Pads · doble click dispara" })
                    .color(if pattern_mode { T.accent } else { T.text_dim }).size(11.0));
                ui.add_space(4.0);
                egui::Grid::new("gm_pads").num_columns(4).spacing(vec2(8.0, 8.0)).show(ui, |ui| {
                    for i in 0..16 {
                        let slot = self.groove_bank * 16 + i;
                        let selected = if pattern_mode { slot == active } else { self.groove_sel_pad == i };
                        let label = if pattern_mode {
                            let nm = &names[slot];
                            if nm.is_empty() { format!("{}{}", if slot < 16 { "A" } else { "B" }, slot % 16 + 1) }
                            else { trunc(nm, 8) }
                        } else {
                            format!("{}\n{}", trunc(PIECES[PAD_ORDER[i]].name, 9), 36 + i)
                        };
                        let fill = if selected { T.accent }
                                   else if pattern_mode && filled[slot] { lighten(T.panel, 0.12) }
                                   else { T.panel };
                        let r = ui.add(egui::Button::new(label).min_size(vec2(84.0, 56.0)).fill(fill));
                        if r.clicked() {
                            if pattern_mode { shared.groove.select_pattern(slot); }
                            else { self.groove_sel_pad = i; }
                        }
                        if r.double_clicked() && !pattern_mode {
                            let cmd = ControlCmd::LivePadOn { pad: i as u8, vel: 100 };
                            let _ = shared.trig_rx.push(cmd.encode());
                        }
                        if (i + 1) % 4 == 0 { ui.end_row(); }
                    }
                });
            });

            ui.separator();

            // ── Columna derecha: rack de 32 patrones, bancos A/B ──
            ui.vertical(|ui| {
                ui.set_min_width(210.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Patrones").color(T.accent).size(12.0));
                    if ui.selectable_label(self.groove_bank == 0, "A").clicked() { self.groove_bank = 0; }
                    if ui.selectable_label(self.groove_bank == 1, "B").clicked() { self.groove_bank = 1; }
                });
                ui.add_space(2.0);
                egui::ScrollArea::vertical().id_source("gm_rack_list")
                    .max_height(260.0).auto_shrink([false, false]).show(ui, |ui| {
                    for i in 0..16 {
                        let slot = self.groove_bank * 16 + i;
                        let tag = format!("{}{}", if slot < 16 { "A" } else { "B" }, slot % 16 + 1);
                        let nm = if names[slot].is_empty() { "—".to_string() } else { trunc(&names[slot], 14) };
                        let on = slot == active;
                        let txt = format!("{} {} {}", if filled[slot] { "●" } else { "○" }, tag, nm);
                        if ui.add(egui::Button::new(txt).fill(if on { T.accent.gamma_multiply(0.35) } else { T.panel })
                            .min_size(vec2(196.0, 20.0))).clicked() {
                            shared.groove.select_pattern(slot);
                        }
                    }
                });
                ui.separator();
                ui.label(egui::RichText::new(format!("Slot {} activo", active + 1)).color(T.text_dim).size(10.0));
                ui.add(egui::TextEdit::singleline(&mut self.groove_pat_name)
                    .hint_text("nombre del patrón").desired_width(190.0));
                ui.horizontal(|ui| {
                    if ui.button("✏ Nombrar").clicked() {
                        let n = self.groove_pat_name.clone();
                        shared.groove.set_pattern_name(&n);
                    }
                    if ui.button("🗑 Vaciar").clicked() { shared.groove.clear_pattern(active); }
                });
                ui.separator();
                if ui.selectable_label(pattern_mode, "Pads = patrones (PATTERN)").clicked() {
                    shared.mk3_pattern_mode.store(!pattern_mode, std::sync::atomic::Ordering::Relaxed);
                }
            });
        });
    }

    fn groove_mixer_tab(&mut self, ui: &mut egui::Ui, shared: &Arc<SessionShared>) {
        use caja_core::pieces::{PAD_ORDER, PIECES};

        // Una sola fuente de verdad: el mismo GrooveKit que usan el MK3 y
        // el hilo de audio. Se lee cada frame, así que el encoder y la cinta
        // del hardware se reflejan aquí al instante.
        let kit = shared.kit.clone();
        let cfg = kit.snapshot_cfg();

        ui.label(egui::RichText::new("Mixer de celdas · Groove Machine")
            .color(T.accent).size(12.0));
        ui.label(egui::RichText::new(
            "MK3: pad selecciona · ENCODER = pan · touchstrip = volumen. Todo se ve y se edita aquí."
        ).color(T.text_dim).size(10.0));
        ui.add_space(4.0);

        egui::ScrollArea::vertical().id_source("gm_mixer_scroll")
            .auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("groove_mixer_grid")
                .num_columns(7)
                .spacing(vec2(8.0, 6.0))
                .show(ui, |ui| {
                    for h in ["Celda", "Gain", "dB", "Pan", "Mute", "Solo", "Nota"] {
                        ui.label(egui::RichText::new(h).color(T.text_dim).size(10.0));
                    }
                    ui.end_row();

                    for i in 0..cfg.len() {
                        let pc = PAD_ORDER[i.min(PAD_ORDER.len() - 1)];
                        let sel = self.groove_sel_pad == i;
                        let label = format!("{} {}", i + 1, trunc(PIECES[pc].name, 12));
                        if ui.selectable_label(sel, label).clicked() {
                            self.groove_sel_pad = i;
                            self.inspector_focus = InspectorFocus::GrooveMixer;
                        }

                        let mut gain = cfg[i].gain_linear();
                        if ui.add(egui::Slider::new(&mut gain, 0.0..=2.0)
                            .show_value(false).text("")).changed()
                        {
                            self.groove_sel_pad = i;
                            kit.set_gain(i, (gain.clamp(0.0, 2.0) * 1000.0).round() as u16);
                        }
                        let db = if gain <= 0.0001 { "-inf".to_string() }
                                 else { format!("{:+.1}", 20.0 * gain.log10()) };
                        ui.label(egui::RichText::new(db).size(10.0).color(T.text_dim));

                        let mut pan = cfg[i].pan_norm();
                        if ui.add(egui::Slider::new(&mut pan, -1.0..=1.0)
                            .show_value(true).text("")).changed()
                        {
                            self.groove_sel_pad = i;
                            kit.set_pan(i, (pan.clamp(-1.0, 1.0) * 1000.0).round() as i16);
                        }

                        let mut mute = cfg[i].mute;
                        if ui.checkbox(&mut mute, "").changed() {
                            self.groove_sel_pad = i;
                            kit.set_mute(i, mute);
                        }
                        let mut solo = cfg[i].solo;
                        if ui.checkbox(&mut solo, "").changed() {
                            self.groove_sel_pad = i;
                            kit.set_solo(i, solo);
                        }

                        ui.label(egui::RichText::new(format!("{}", 36 + i))
                            .size(10.0).color(T.text_dim));
                        ui.end_row();
                    }
                });
        });
    }
    fn groove_step_tab(&mut self, ui: &mut egui::Ui, shared: &Arc<SessionShared>) {
        use caja_core::pieces::{PAD_ORDER, PIECES};
        use caja_core::pattern::STEPS_PER_BAR;

        let (pattern, notes) = match &self.groove_snap {
            Some(s) => (s.pattern.clone(), s.notes),
            None => { ui.label("Cargando patrón…"); return; }
        };

        let bars = pattern.bars.max(1);
        let total_steps = pattern.steps();
        let rack_like = self.groove_view == 1 || self.groove_event;
        // EVENT y Rack: 1 compás · Piano Roll: 2 compases por página
        let per_page = if rack_like { 1usize } else { 2usize };
        let pages = bars.div_ceil(per_page).max(1);

        // Seguir el compás/página que está sonando.
        let abs_step_now = shared.caja_sched.current_abs_step.load(
            std::sync::atomic::Ordering::Relaxed,
        );
        let playing = shared.caja_sched.enabled.load(
            std::sync::atomic::Ordering::Relaxed,
        );

        if self.groove_follow_playhead && playing && abs_step_now != u32::MAX {
            let abs_bar = (abs_step_now as usize / STEPS_PER_BAR)
                .min(bars.saturating_sub(1));

            if rack_like {
                self.groove_bar = abs_bar;
            } else {
                self.groove_page = (abs_bar / per_page)
                    .min(pages.saturating_sub(1));
            }
        }

        shared.groove_bar.store(self.groove_bar, std::sync::atomic::Ordering::Relaxed);
        shared.groove_page.store(self.groove_page, std::sync::atomic::Ordering::Relaxed);
        shared.groove_view.store(self.groove_view as u8, std::sync::atomic::Ordering::Relaxed);
        self.groove_bar = self.groove_bar.min(bars - 1);
        self.groove_page = self.groove_page.min(pages - 1);

        let first = if rack_like { self.groove_bar * STEPS_PER_BAR }
                    else { self.groove_page * per_page * STEPS_PER_BAR };
        let cols = (per_page * STEPS_PER_BAR).min(total_steps.saturating_sub(first));

        let abs_step_now = shared.caja_sched.current_abs_step.load(std::sync::atomic::Ordering::Relaxed);
        let playing = shared.caja_sched.enabled.load(std::sync::atomic::Ordering::Relaxed);

        // ── Cabecera ──
        ui.horizontal(|ui| {
            if ui.selectable_label(self.groove_view == 0 && !self.groove_event, "Piano Roll").clicked() {
                self.groove_view = 0;
                shared.groove_view.store(0, std::sync::atomic::Ordering::Relaxed);
                shared.mk3_event_mode.store(false, std::sync::atomic::Ordering::Relaxed);
            }
            if ui.selectable_label(rack_like, "Rack").clicked() {
                self.groove_view = 1;
                shared.groove_view.store(1, std::sync::atomic::Ordering::Relaxed);
            }
            if self.groove_event {
                ui.label(egui::RichText::new("EVENT").color(T.accent).strong().size(11.0));
            }
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Modo").color(T.text_dim).size(10.0));
                if ui.selectable_label(self.groove_follow_playhead, "Seguir").clicked() {
                    self.groove_follow_playhead = true;
                    shared.groove_follow_playhead.store(true, std::sync::atomic::Ordering::Relaxed);
                    shared.mk3_follow_playhead.store(true, std::sync::atomic::Ordering::Relaxed);
                    let abs_step_now = shared.caja_sched.current_abs_step.load(
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    let playing = shared.caja_sched.enabled.load(
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    if playing && abs_step_now != u32::MAX {
                        let bars = self.groove_snap.as_ref().map(|s| s.pattern.bars).unwrap_or(1).max(1);
                        let per_page = if self.groove_view == 1 || self.groove_event { 1usize } else { 2usize };
                        let pages = bars.div_ceil(per_page).max(1);
                        let abs_bar = (abs_step_now as usize / STEPS_PER_BAR).min(bars.saturating_sub(1));
                        if self.groove_view == 1 || self.groove_event {
                            self.groove_bar = abs_bar;
                        } else {
                            self.groove_page = (abs_bar / per_page).min(pages.saturating_sub(1));
                        }
                    }
                }
                if ui.selectable_label(!self.groove_follow_playhead, "Editar").clicked() {
                    self.groove_follow_playhead = false;
                    shared.groove_follow_playhead.store(false, std::sync::atomic::Ordering::Relaxed);
                    shared.mk3_follow_playhead.store(false, std::sync::atomic::Ordering::Relaxed);
                }
            });
            ui.separator();
            if ui.button("◀").clicked() {
                if rack_like { self.groove_bar = self.groove_bar.saturating_sub(1);
                shared.groove_bar.store(self.groove_bar, std::sync::atomic::Ordering::Relaxed);
                shared.groove_page.store(self.groove_page, std::sync::atomic::Ordering::Relaxed);
                shared.groove_view.store(self.groove_view as u8, std::sync::atomic::Ordering::Relaxed); }
                else { self.groove_page = self.groove_page.saturating_sub(1);
                shared.groove_bar.store(self.groove_bar, std::sync::atomic::Ordering::Relaxed);
                shared.groove_page.store(self.groove_page, std::sync::atomic::Ordering::Relaxed);
                shared.groove_view.store(self.groove_view as u8, std::sync::atomic::Ordering::Relaxed); }
            }
            if rack_like {
                ui.label(format!("Compás {}/{}", self.groove_bar + 1, bars));
            } else {
                let a = self.groove_page * per_page + 1;
                let b = (a + per_page - 1).min(bars);
                ui.label(format!("Compases {}-{} / {} · pág {}/{}", a, b, bars, self.groove_page + 1, pages));
            }
            if ui.button("▶").clicked() {
                if rack_like { self.groove_bar = (self.groove_bar + 1).min(bars - 1);
                shared.groove_bar.store(self.groove_bar, std::sync::atomic::Ordering::Relaxed);
                shared.groove_page.store(self.groove_page, std::sync::atomic::Ordering::Relaxed);
                shared.groove_view.store(self.groove_view as u8, std::sync::atomic::Ordering::Relaxed); }
                else { self.groove_page = (self.groove_page + 1).min(pages - 1);
                shared.groove_bar.store(self.groove_bar, std::sync::atomic::Ordering::Relaxed);
                shared.groove_page.store(self.groove_page, std::sync::atomic::Ordering::Relaxed);
                shared.groove_view.store(self.groove_view as u8, std::sync::atomic::Ordering::Relaxed); }
            }
            if ui.button("↻").on_hover_text("Ir al compás actual").clicked() {
                let abs_step_now = shared.caja_sched.current_abs_step.load(
                    std::sync::atomic::Ordering::Relaxed,
                );
                if abs_step_now != u32::MAX {
                    let abs_bar = (abs_step_now as usize / STEPS_PER_BAR)
                        .min(bars.saturating_sub(1));
                    if rack_like {
                        self.groove_bar = abs_bar;
                    } else {
                        self.groove_page = (abs_bar / per_page)
                            .min(pages.saturating_sub(1));
                    }
                }
            }
            ui.separator();
            if !rack_like {
                if ui.selectable_label(self.groove_show_all, "Todas").clicked() {
                    self.groove_show_all = !self.groove_show_all;
                }
            }
            if ui.button("🔄").on_hover_text("Regenerar").clicked() { shared.groove.reroll(true); }
        });
        ui.label(egui::RichText::new(
            "Clic: golpe · Arrastre vertical: velocidad · Clic derecho: ghost · SHIFT+◀▶ en MK3: cambiar vista"
        ).color(T.text_dim).size(10.0));
        ui.add_space(4.0);

        // ── Filas: en Rack/EVENT solo la celda elegida; en Roll todas ──
        let used = |pc: usize| (0..total_steps).any(|s| pattern.cell[pc][s].on());
        let rows: Vec<usize> = if rack_like {
            vec![self.groove_sel_pad.min(PAD_ORDER.len() - 1)]
        } else {
            (0..PAD_ORDER.len())
                .filter(|&r| self.groove_show_all || used(PAD_ORDER[r]) || r == self.groove_sel_pad)
                .collect()
        };

        let name_w = 104.0;
        let rh = if rack_like { 46.0 } else { 17.0 };
        let avail_w = (ui.available_width() - name_w - 12.0).max(80.0);
        let cw = (avail_w / cols.max(1) as f32).floor().max(if rack_like { 18.0 } else { 7.0 });

        let scroll_id = if rack_like { "gm_rack" } else { "gm_roll" };
        egui::ScrollArea::both().id_source(scroll_id).auto_shrink([false, false]).show(ui, |ui| {
            let head_h = 16.0;
            let grid_h = rows.len() as f32 * rh;
            let grid_w = cw * cols as f32;
            let total = vec2(name_w + grid_w + 4.0, head_h + grid_h + 4.0);
            let (outer, _) = ui.allocate_exact_size(total, egui::Sense::hover());
            let paint = ui.painter_at(outer);
            let x0 = outer.min.x + name_w;
            let y0 = outer.min.y + head_h;
            let grid = Rect::from_min_size(pos2(x0, y0), vec2(grid_w, grid_h));

            for c in 0..cols {
                let st = first + c;
                let is_bar = st % STEPS_PER_BAR == 0;
                let is_beat = st % 4 == 0;
                let lbl = if is_bar { format!("▸{}", st / STEPS_PER_BAR + 1) }
                          else if is_beat { format!("{}", st % STEPS_PER_BAR + 1) }
                          else { "·".into() };
                let col = if is_bar { egui::Color32::WHITE }
                          else if is_beat { T.text_dim }
                          else { T.text_dim.gamma_multiply(0.5) };
                paint.text(pos2(x0 + c as f32 * cw + cw * 0.5, outer.min.y + head_h * 0.5),
                    Align2::CENTER_CENTER, lbl, FontId::proportional(9.0), col);
            }

            for (r, &pad) in rows.iter().enumerate() {
                let pc = PAD_ORDER[pad];
                let ry = y0 + r as f32 * rh;
                let sel = pad == self.groove_sel_pad;
                let row_rect = Rect::from_min_size(pos2(outer.min.x, ry), vec2(name_w - 2.0, rh));
                if r % 2 == 1 {
                    paint.rect_filled(Rect::from_min_size(pos2(x0, ry), vec2(grid_w, rh)),
                        0.0, T.panel.gamma_multiply(0.35));
                }
                if sel { paint.rect_filled(row_rect, 2.0, T.accent.gamma_multiply(0.22)); }
                let hue = PAD_PALETTE[pad % PAD_PALETTE.len()];
                paint.rect_filled(Rect::from_min_size(pos2(outer.min.x, ry + 2.0), vec2(3.0, rh - 4.0)), 1.0, hue);
                paint.text(pos2(outer.min.x + 7.0, ry + rh * 0.5), Align2::LEFT_CENTER,
                    trunc(PIECES[pc].name, 13), FontId::proportional(10.0),
                    if sel { T.accent } else { lighten(T.text_dim, 0.45) });
                paint.text(pos2(outer.min.x + name_w - 6.0, ry + rh * 0.5), Align2::RIGHT_CENTER,
                    format!("{}", notes[pc]), FontId::proportional(9.0), T.text_dim);

                for c in 0..cols {
                    let st = first + c;
                    if st >= total_steps { break; }
                    let cell = pattern.cell[pc][st];
                    let slot = Rect::from_min_size(pos2(x0 + c as f32 * cw + 1.0, ry + 1.0), vec2(cw - 2.0, rh - 2.0));
                    let is_active = playing
                        && rack_like
                        && abs_step_now != u32::MAX
                        && abs_step_now as usize == st;
                    let slot_fill = if is_active {
                        T.accent.gamma_multiply(0.18)
                    } else {
                        T.panel
                    };
                    paint.rect_filled(slot, 2.0, slot_fill);
                    if cell.on() {
                        let h = (slot.height() * f32::from(cell.vel) / 127.0).max(3.0);
                        let bar = Rect::from_min_max(pos2(slot.min.x, slot.max.y - h), slot.max);
                        if cell.ghost { paint.rect_stroke(bar, 2.0, Stroke::new(1.5, hue)); }
                        else { paint.rect_filled(bar, 2.0, hue); }
                    }
                    if is_active {
                        paint.rect_stroke(slot, 2.0, Stroke::new(1.5, T.accent));
                    }
                }
                for c in 0..=cols {
                    let st = first + c;
                    if st % STEPS_PER_BAR == 0 {
                        paint.rect_filled(Rect::from_min_size(pos2(x0 + c as f32 * cw - 1.0, y0), vec2(2.0, grid_h)),
                            0.0, T.text_dim.gamma_multiply(0.7));
                    } else if st % 4 == 0 {
                        paint.rect_filled(Rect::from_min_size(pos2(x0 + c as f32 * cw, y0), vec2(1.0, grid_h)),
                            0.0, T.text_dim.gamma_multiply(0.3));
                    }
                }
            }

            if playing && rack_like && abs_step_now != u32::MAX {
                let abs_step = abs_step_now as usize;
                if abs_step >= first && abs_step < first + cols {
                    let hx = x0 + (abs_step - first) as f32 * cw;
                    paint.rect_filled(Rect::from_min_size(pos2(hx, outer.min.y), vec2(cw, head_h + grid_h)),
                        0.0, T.accent.gamma_multiply(0.13));
                    paint.rect_filled(Rect::from_min_size(pos2(hx, outer.min.y), vec2(2.0, head_h + grid_h)), 0.0, T.accent);
                }
            }

            let cell_at = |pos: Pos2| -> Option<(usize, usize)> {
                if !grid.contains(pos) { return None; }
                let c = ((pos.x - x0) / cw) as usize;
                let r = ((pos.y - y0) / rh) as usize;
                let pad = *rows.get(r)?;
                let st = first + c;
                if st >= total_steps { return None; }
                Some((PAD_ORDER[pad], st))
            };
            let resp = ui.interact(grid, ui.id().with("gm_grid"), Sense::click_and_drag());
            let names = Rect::from_min_size(outer.min, vec2(name_w, head_h + grid_h));
            let nresp = ui.interact(names, ui.id().with("gm_names"), Sense::click());
            if nresp.clicked() {
                if let Some(pos) = nresp.interact_pointer_pos() {
                    let r = ((pos.y - y0) / rh) as usize;
                    if let Some(&pad) = rows.get(r) { self.groove_sel_pad = pad; }
                }
            }
            if resp.drag_started_by(egui::PointerButton::Primary) {
                if let Some((pc, st)) = ui.input(|i| i.pointer.press_origin()).and_then(cell_at) {
                    let cell = pattern.cell[pc][st];
                    self.groove_drag = Some((pc, st, i32::from(cell.vel), cell.on()));
                    self.groove_sel_pad = PAD_ORDER.iter().position(|&x| x == pc).unwrap_or(0);
                }
            }
            if resp.dragged_by(egui::PointerButton::Primary) {
                if let (Some(d), Some(pos), Some(o)) = (self.groove_drag, resp.interact_pointer_pos(),
                                                        ui.input(|i| i.pointer.press_origin())) {
                    if d.3 { shared.groove.set_cell_vel(d.0, d.1, d.2 + ((o.y - pos.y) * 1.3) as i32); }
                }
            }
            if resp.drag_stopped() { self.groove_drag = None; }
            if resp.clicked_by(egui::PointerButton::Primary) {
                if let Some((pc, st)) = resp.interact_pointer_pos().and_then(cell_at) {
                    shared.groove.toggle_cell(pc, st);
                    self.groove_sel_pad = PAD_ORDER.iter().position(|&x| x == pc).unwrap_or(0);
                }
            }
            if resp.secondary_clicked() {
                if let Some((pc, st)) = resp.interact_pointer_pos().and_then(cell_at) {
                    shared.groove.toggle_ghost(pc, st);
                }
            }
        });
    }

    fn global_browser_window(&mut self, ctx: &egui::Context, shared: &Arc<SessionShared>) {
        let mut open = self.windows.browser_visible;

        egui::Window::new("📁 Browser")
            .open(&mut open)
            .constrain(false)
            .max_size([8000.0, 8000.0])
            .default_size([380.0, 560.0])
            .min_size([280.0, 360.0])
            .resizable(true)
            .frame(Self::window_frame())
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("🏠").clicked() {
                        self.browser_path = std::path::PathBuf::from(
                            std::env::var("HOME").unwrap_or_else(|_| "/".into())
                        );
                    }
                    if ui.button("⬆").clicked() {
                        if let Some(parent) = self.browser_path.parent() {
                            self.browser_path = parent.to_path_buf();
                        }
                    }
                    ui.add(
                        egui::TextEdit::singleline(&mut self.browser_filter)
                            .desired_width(140.0)
                            .hint_text("filtrar")
                    );
                    ui.checkbox(&mut self.browser_favorites_only, "⭐");
                });

                ui.label(egui::RichText::new(self.browser_path.display().to_string()).color(T.text_dim).size(10.0));
                ui.separator();

                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    let Ok(entries) = std::fs::read_dir(&self.browser_path) else {
                        ui.label("No se pudo leer la carpeta.");
                        return;
                    };

                    let mut items: Vec<_> = entries.flatten().collect();
                    items.sort_by_key(|e| (e.path().is_file(), e.file_name()));

                    for entry in items {
                        let path = entry.path();
                        let name = entry.file_name().to_string_lossy().to_string();

                        if !self.browser_filter.is_empty()
                            && !name.to_lowercase().contains(&self.browser_filter.to_lowercase())
                        {
                            continue;
                        }

                        if self.browser_favorites_only {
                            // TODO: base de favoritos. Por ahora no filtra nada.
                        }

                        if path.is_dir() {
                            if ui.button(format!("📁 {}", name)).clicked() {
                                self.browser_path = path;
                            }
                        } else if is_audio_file(&path) {
                            let resp = ui.button(format!("🎵 {}", name));
                            if resp.clicked() {
                                let (track, scene) = {
                                    let st = shared.state.lock().unwrap();
                                    (st.selected_track, st.selected_scene)
                                };

                                let req = lanzadera_engine::shared::LoadRequest {
                                    track,
                                    scene,
                                    path: path.to_string_lossy().to_string(),
                                };

                                if let Ok(mut q) = shared.load_queue.lock() {
                                    q.push_back(req);
                                }

                                shared.rescan.store(true, std::sync::atomic::Ordering::Relaxed);
                                self.msg = format!("Cargando {} en Pista {} · Escena {}", name, track + 1, scene + 1);
                            }
                        }
                    }
                });
            });

        self.windows.browser_visible = open;
    }

    fn global_inspector_window(&mut self, ctx: &egui::Context, shared: &Arc<SessionShared>) {
        let mut open = self.windows.inspector_visible;

        egui::Window::new("🔍 Inspector")
            .open(&mut open)
            .constrain(false)
            .max_size([8000.0, 8000.0])
            .default_size([300.0, 560.0])
            .min_size([240.0, 320.0])
            .resizable(true)
            .frame(Self::window_frame())
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                // El inspector sigue el FOCO, no la visibilidad de la ventana GM.
                // Sin foco explícito se conserva el comportamiento anterior.
                let groove_focused = match self.inspector_focus {
                    InspectorFocus::GroovePad
                    | InspectorFocus::GrooveMixer
                    | InspectorFocus::GrooveStep => true,
                    InspectorFocus::Clip | InspectorFocus::Track => false,
                    _ => self.windows.groove_machine_visible,
                };
                if groove_focused
                    && shared.groove_sub_mode.load(std::sync::atomic::Ordering::Relaxed) == 3
                {
                    // CHORDS: inspector de biblioteca (dado, locks, muestra).
                    self.inspector_groove_chords(ui, shared);
                } else if groove_focused {
                    match self.groove_tab {
                        GrooveTab::Pad => self.inspector_groove_pad(ui, shared),
                        GrooveTab::Mixer => self.inspector_groove_cell(ui, shared),
                        GrooveTab::Step => {
                            if self.groove_view == 1 || self.groove_event {
                                self.inspector_groove_cell(ui, shared);
                            } else {
                                self.inspector_groove_step(ui, shared);
                            }
                        }
                    }
                } else {
                    // Contexto Clip Launch: inspector completo (clip, EQ, plugin
                    // CLAP y sus parámetros), no solo etiquetas de transporte.
                    self.inspector_clip_body(ui, shared);
                }
                });
            });

        self.windows.inspector_visible = open;
    }

    fn mixer_window(&mut self, ctx: &egui::Context, shared: &Arc<SessionShared>, audio_on: bool) {
        let mut open = self.windows.mixer_visible;

        egui::Window::new("🎚 Mixer")
            .open(&mut open)
            .constrain(false)
            .max_size([8000.0, 8000.0])
            .default_size([900.0, 260.0])
            .min_size([420.0, 160.0])
            .resizable(true)
            .frame(Self::window_frame())
            .show(ctx, |ui| {
                // self.mixer ya trae su ScrollArea::horizontal interno;
                // la ventana es resizable en ambos ejes gracias a min_size + resizable(true).
                self.mixer(ui, shared, audio_on);
            });

        self.windows.mixer_visible = open;
    }
}

#[allow(dead_code)] // helper reservado para el disparo de pads por MIDI
fn shared_trigger_push(shared: &Arc<SessionShared>, cmd: ControlCmd) -> Result<(), ()> {
    // Si Spsc::push no existe con esta firma, este helper se ajustará en el siguiente parche.
    // Por ahora intentamos usar el canal de triggers existente.
    shared.trig_rx.push(cmd.encode());
    Ok(())
}

fn is_audio_file(path: &std::path::Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => matches!(
            ext.to_ascii_lowercase().as_str(),
            "wav" | "flac" | "ogg" | "mp3" | "aif" | "aiff"
        ),
        None => false,
    }
}

// ════════════════════════════════════════════════════════════════════════
//  Fase 4B.2 — Inspector de Groove Machine conectado al modelo real
// ════════════════════════════════════════════════════════════════════════

/// Etiquetas de los 4 tamaños de patrón (BARS_CHOICES de caja-core).
const GM_BARS: [&str; 4] = ["1", "2", "4", "8"];

impl SessionView {
    fn inspector_groove_step(&mut self, ui: &mut egui::Ui, shared: &Arc<SessionShared>) {
        // Copias ligeras del snapshot (ParamSnap es Copy; el resto son 2 Strings).
        let (mut p, genre_name, reading, active) = match &self.groove_snap {
            Some(s) => (s.p, s.genre_name.clone(), s.reading.clone(), s.active),
            None => { ui.label("Cargando motor de Groove Machine…"); return; }
        };

        ui.heading("Patrón / Step");
        ui.label(egui::RichText::new(format!("Slot {} · {}", active + 1, genre_name))
            .color(T.accent).size(11.0));
        ui.separator();

        // ── Groove: swing / acento / humanize / densidad ──
        ui.label(egui::RichText::new("Groove").color(T.accent).size(12.0));
        if ui.add(egui::Slider::new(&mut p.swing, 0..=100).text("Swing %")).changed() {
            shared.groove.set_swing(p.swing);
        }
        if ui.add(egui::Slider::new(&mut p.accent, 0..=100).text("Acento %")).changed() {
            shared.groove.set_accent(p.accent);
        }
        if ui.add(egui::Slider::new(&mut p.human, 0..=100).text("Humanize %")).changed() {
            shared.groove.set_human(p.human);
        }
        if ui.add(egui::Slider::new(&mut p.density, 0..=100).text("Densidad %")).changed() {
            shared.groove.set_density(p.density);
        }

        ui.separator();

        // ── Extensión del patrón (VARIATION) ──
        ui.label(egui::RichText::new("Compases").color(T.accent).size(12.0));
        ui.horizontal(|ui| {
            for (i, lbl) in GM_BARS.iter().enumerate() {
                let on = p.bars_idx == i as i32;
                if ui.add(egui::Button::new(*lbl)
                    .fill(if on { T.accent } else { T.panel })).clicked() {
                    shared.groove.set_bars_idx(i as i32);
                }
            }
        });

        ui.separator();

        // ── Semilla ──
        ui.label(egui::RichText::new("Semilla").color(T.accent).size(12.0));
        ui.horizontal(|ui| {
            if ui.add(egui::DragValue::new(&mut p.seed).speed(1.0)).changed() {
                shared.groove.set_seed(p.seed);
            }
            if ui.button("🎲 Dado").clicked() { shared.groove.randomize_seed(); }
        });

        ui.separator();

        // ── Género: combo + siguiente/anterior (FOLLOW) ──
        ui.label(egui::RichText::new("Género").color(T.accent).size(12.0));
        let genres = self.groove_genres.clone();
        let cur_label = genres.get(p.genre.max(0) as usize)
            .cloned().unwrap_or_else(|| genre_name.clone());
        let families: Vec<String> = self.groove_snap.as_ref()
            .map(|s| s.genre_families.clone())
            .unwrap_or_default();
        egui::ComboBox::from_id_source("gm_genre")
            .selected_text(cur_label)
            .width(180.0)
            .height(420.0)
            .show_ui(ui, |ui| {
                // Agrupar por familia sin alterar los índices: el índice es el
                // valor del parámetro Género y lo usan los proyectos guardados
                // y el FOLLOW del MK3.
                if families.len() != genres.len() {
                    // Snapshot sin datos de familia: lista plana como antes.
                    for (i, g) in genres.iter().enumerate() {
                        if ui.selectable_label(p.genre == i as i32, g).clicked() {
                            shared.groove.set_genre(i);
                        }
                    }
                } else {
                    let mut orden: Vec<&str> = Vec::new();
                    for fam in &families {
                        if !orden.contains(&fam.as_str()) { orden.push(fam.as_str()); }
                    }
                    for fam in orden {
                        ui.separator();
                        ui.label(egui::RichText::new(fam).color(T.accent).size(10.0).strong());
                        for (i, g) in genres.iter().enumerate() {
                            if families[i].as_str() != fam { continue; }
                            if ui.selectable_label(p.genre == i as i32, g).clicked() {
                                shared.groove.set_genre(i);
                            }
                        }
                    }
                }
            });
        ui.horizontal(|ui| {
            if ui.button("◀ Anterior").clicked() { shared.groove.cycle_genre(-1); }
            if ui.button("Siguiente ▶").clicked() { shared.groove.cycle_genre(1); }
        });

        ui.separator();

        // ── Generar desde frase (ERASE) / Regenerar (RESTART) ──
        ui.label(egui::RichText::new("Generar").color(T.accent).size(12.0));
        ui.add(egui::TextEdit::singleline(&mut self.groove_phrase)
            .hint_text("ej: techno 132 bpm con fill 4 compases")
            .desired_width(f32::INFINITY));
        ui.horizontal(|ui| {
            if ui.button("✏ Generar").clicked() {
                let ph = self.groove_phrase.clone();
                shared.groove.generate(&ph);
            }
            if ui.button("🔄 Regenerar").clicked() { shared.groove.reroll(true); }
        });

        ui.separator();

        // ── Export (STAR) ──
        if ui.button("💾 Exportar .mid").clicked() {
            self.msg = match shared.groove.export_midi(false, false) {
                Ok(path) => format!("MIDI guardado en {}", path.display()),
                Err(e) => format!("⚠ Export: {}", e),
            };
        }

        // ── Lectura del patrón (lo que Caja describe en palabras) ──
        if !reading.is_empty() {
            ui.separator();
            ui.label(egui::RichText::new("Lectura").color(T.accent).size(12.0));
            ui.label(egui::RichText::new(reading).color(T.text_dim).size(10.0));
        }
    }
}

// ════════════════════════════════════════════════════════════════════════
//  Fase 4C.2 — Inspector de Groove Machine: Pad + Biblioteca
//  Equivale a la pestaña Library de Caja: carpeta raíz, RAM, duración
//  máxima y bandeja de "Sin clasificar" con asignación manual.
// ════════════════════════════════════════════════════════════════════════

impl SessionView {
    fn inspector_groove_pad(&mut self, ui: &mut egui::Ui, _shared: &Arc<SessionShared>) {
        use caja_core::pieces::{PAD_ORDER, PIECES};
        use lanzadera_engine::caja_drums::library_state;

        let pad_idx = self.groove_sel_pad.min(24);

        ui.heading(format!("Pad {} · {}", pad_idx + 1, PIECES[PAD_ORDER[pad_idx]].name));
        ui.separator();

        // ── Parámetros del pad (locales hasta conectar KitShared) ──
        ui.label(egui::RichText::new("Gain").color(T.text_dim).size(11.0));
        ui.add(egui::Slider::new(&mut self.groove_pad_gain[pad_idx], 0.0..=2.0).text(""));
        ui.label(egui::RichText::new("Pan").color(T.text_dim).size(11.0));
        ui.add(egui::Slider::new(&mut self.groove_pad_pan[pad_idx], -1.0..=1.0).text(""));
        ui.checkbox(&mut self.groove_pad_mute[pad_idx], "Mute");

        ui.separator();

        // ══ BIBLIOTECA (pestaña Library de Caja) ══
        ui.label(egui::RichText::new("📁 Biblioteca de muestras").color(T.accent).size(13.0));

        let lib = library_state::shared();
        let mut g = lib.lock().unwrap_or_else(|e| e.into_inner());

        // ── Carpeta raíz ──
        ui.label(egui::RichText::new("Carpeta de one-shots").color(T.text_dim).size(11.0));
        ui.add(egui::TextEdit::singleline(&mut self.groove_root_input)
            .hint_text(&g.root)
            .desired_width(f32::INFINITY));
        ui.horizontal(|ui| {
            if ui.button("✔ Aplicar").clicked() {
                let r = self.groove_root_input.clone();
                if !r.trim().is_empty() { g.set_root(&r); }
            }
            if ui.button("📁 Usar la del Browser").clicked() {
                let r = self.browser_path.to_string_lossy().to_string();
                self.groove_root_input = r.clone();
                g.set_root(&r);
            }
            if ui.button("🔄").on_hover_text("Re-escanear").clicked() {
                g.request_rescan();
            }
        });
        ui.label(egui::RichText::new(format!("Activa: {}", g.root))
            .color(T.text_dim).size(9.0));

        ui.separator();

        // ── Límites: RAM y duración ──
        let mut cap = g.bank_cap as i32;
        ui.label(egui::RichText::new("Muestras por pad (RAM)").color(T.text_dim).size(11.0));
        if ui.add(egui::Slider::new(&mut cap, 8..=128)).changed() {
            g.set_bank_cap(cap as usize);
        }

        let mut maxlen = g.max_len_ms as i32;
        ui.label(egui::RichText::new("Duración máxima (ms) — excluye loops").color(T.text_dim).size(11.0));
        if ui.add(egui::Slider::new(&mut maxlen, 0..=8000).text("ms")).changed() {
            g.set_max_len_ms(maxlen.max(0) as u32);
        }
        ui.label(egui::RichText::new("0 = sin límite · ~1500-2000 ms deja solo one-shots")
            .color(T.text_dim).size(9.0));

        ui.separator();

        // ── Estadísticas ──
        ui.label(egui::RichText::new(
            format!("{} archivos · {} clasificadas · {} sin clasificar",
                g.files.len(), g.total_classified(), g.unclassified().len())
        ).size(10.0));
        ui.label(egui::RichText::new(
            format!("{} cargadas · {} excluidas por duración",
                g.total_loaded(), g.skipped_long)
        ).color(T.text_dim).size(10.0));

        ui.separator();

        // ── Muestras del pad seleccionado ──
        let cell = g.lib.cells.get(pad_idx).cloned().unwrap_or_default();
        egui::CollapsingHeader::new(format!("Muestras de este pad ({})", cell.len()))
            .default_open(false)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().max_height(150.0)
                    .auto_shrink([false, false]).show(ui, |ui| {
                    for rel in cell.iter().take(200) {
                        ui.label(egui::RichText::new(rel).size(9.0));
                    }
                });
            });

        // ── Bandeja de SIN CLASIFICAR con asignación manual ──
        let total_unclass = g.unclassified().len();
        egui::CollapsingHeader::new(format!("⚠ Sin clasificar ({})", total_unclass))
            .default_open(false)
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Elige el pad donde debe ir cada muestra.")
                    .color(T.text_dim).size(9.0));

                let row_h = 22.0;
                let mut action: Option<(String, usize)> = None;

                egui::ScrollArea::vertical()
                    .max_height(240.0)
                    .auto_shrink([false, false])
                    .show_rows(ui, row_h, total_unclass, |ui, range| {
                        for i in range {
                            let rel = g.unclassified()[i].clone();
                            let name = std::path::Path::new(&rel)
                                .file_name().map(|s| s.to_string_lossy().to_string())
                                .unwrap_or_else(|| rel.clone());
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(trunc(&name, 26)).size(10.0))
                                    .on_hover_text(&rel);
                                egui::ComboBox::from_id_source(("gm_assign", i))
                                    .selected_text("pad ▾")
                                    .width(84.0)
                                    .show_ui(ui, |ui| {
                                        for p in 0..16 {
                                            let lbl = format!("{} {}", p + 1, PIECES[PAD_ORDER[p]].name);
                                            if ui.selectable_label(false, lbl).clicked() {
                                                action = Some((rel.clone(), p));
                                            }
                                        }
                                    });
                            });
                        }
                    });

                if let Some((rel, pad)) = action {
                    g.assign_to_pad(&rel, pad);
                }
            });

        // ── Asignaciones manuales activas ──
        let n_assign = g.assign.len();
        if n_assign > 0 {
            egui::CollapsingHeader::new(format!("Asignaciones manuales ({})", n_assign))
                .default_open(false)
                .show(ui, |ui| {
                    let mut remove: Option<String> = None;
                    egui::ScrollArea::vertical().max_height(150.0)
                        .auto_shrink([false, false]).show(ui, |ui| {
                        for (rel, pad) in g.assign.clone() {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(
                                    format!("→ pad {} · {}", pad + 1, trunc(&rel, 24))).size(9.0));
                                if ui.small_button("✕").clicked() { remove = Some(rel); }
                            });
                        }
                    });
                    if let Some(rel) = remove { g.unassign(&rel); }
                });
        }
    }
}

/// Paleta cíclica para las filas del step sequencer (una por pad).
/// Evita depender de Theme::group de Caja, que usa otra escala de colores.
const PAD_PALETTE: [egui::Color32; 8] = [
    egui::Color32::from_rgb(232,  92,  92),  // rojo
    egui::Color32::from_rgb(240, 152,  72),  // naranja
    egui::Color32::from_rgb(232, 208,  88),  // amarillo
    egui::Color32::from_rgb(140, 216, 108),  // lima
    egui::Color32::from_rgb( 96, 208, 176),  // menta
    egui::Color32::from_rgb( 96, 176, 232),  // cian
    egui::Color32::from_rgb(152, 132, 232),  // violeta
    egui::Color32::from_rgb(224, 120, 200),  // magenta
];

// ════════════════════════════════════════════════════════════════════════
//  Inspector de celda de batería (modo Mixer, y Step en vista Rack/EVENT)
// ════════════════════════════════════════════════════════════════════════
impl SessionView {
    fn inspector_groove_cell(&mut self, ui: &mut egui::Ui, _shared: &Arc<SessionShared>) {
        use caja_core::pieces::{PAD_ORDER, PIECES};
        use lanzadera_engine::caja_drums::library_state;

        let pad = self.groove_sel_pad.min(24);
        let pc = PAD_ORDER[pad];
        let note = self.groove_snap.as_ref().map(|s| s.notes[pc]).unwrap_or(36 + pad as u8);

        ui.heading(format!("Pad {} · {}", pad + 1, PIECES[pc].name));
        ui.label(egui::RichText::new(format!("Pieza {} · nota MIDI {}", pc, note))
            .color(T.text_dim).size(10.0));
        ui.separator();

        // ── Mezcla de la celda (conectada al GrooveKit) ──
        ui.label(egui::RichText::new("Mezcla").color(T.accent).size(12.0));

        let kit = _shared.kit.clone();
        let cfg = kit.snapshot_cfg();
        let c = cfg[pad].clone();
        let mut gain_db = (c.gain_linear() - 1.0) * 12.0; // 0 dB por defecto
        if ui.add(egui::Slider::new(&mut gain_db, -60.0..=12.0)
            .text("Gain (dB)").suffix(" dB")).changed() {
            let lin = (10.0f32).powf(gain_db / 20.0);
            kit.set_gain(pad, (lin * 1000.0).round() as u16);
        }

        let mut pan = c.pan_norm();
        if ui.add(egui::Slider::new(&mut pan, -1.0..=1.0)
            .text("Pan").suffix("")).changed() {
            kit.set_pan(pad, (pan * 1000.0).round() as i16);
        }

        ui.horizontal(|ui| {
            let mut mute = c.mute;
            if ui.checkbox(&mut mute, "Mute").changed() { kit.set_mute(pad, mute); }
            let mut solo = c.solo;
            if ui.checkbox(&mut solo, "Solo").changed() { kit.set_solo(pad, solo); }
        });

        ui.separator();

        // ── Muestras asignadas a este pad ──
        let lib = library_state::shared();
        let g = lib.lock().unwrap_or_else(|e| e.into_inner());
        let cell = g.lib.cells.get(pad).cloned().unwrap_or_default();
        let loaded = g.loaded.get(pad).copied().unwrap_or(0);
        ui.label(egui::RichText::new(format!("Muestras · {} cargadas de {}", loaded, cell.len()))
            .color(T.accent).size(12.0));
        egui::ScrollArea::vertical().id_source("gm_cell_samples")
            .max_height(140.0).auto_shrink([false, false]).show(ui, |ui| {
            for rel in cell.iter().take(120) {
                let name = std::path::Path::new(rel)
                    .file_name().map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| rel.clone());
                ui.label(egui::RichText::new(trunc(&name, 30)).size(9.0))
                    .on_hover_text(rel);
            }
            if cell.is_empty() {
                ui.label(egui::RichText::new("Sin muestras asignadas").color(T.text_dim).size(10.0));
            }
        });

        ui.separator();

        // ── Slot y semilla (CHORDS-style: dado) ──
        ui.label(egui::RichText::new("Muestra actual").color(T.accent).size(12.0));
        let live = kit.snapshot_live();
        let slot = live.get(pad).map(|l| l.slot as usize).unwrap_or(0);
        ui.label(egui::RichText::new(format!("Slot {}/{}", slot + 1, 8))
            .color(T.text_dim).size(10.0));
        ui.horizontal(|ui| {
            if ui.button("🎲 Dado").clicked() {
                kit.roll_slot(pad);
            }
            for s in 0..8u8 {
                if ui.selectable_label(slot == s as usize, format!("{}", s + 1)).clicked() {
                    kit.set_active_slot(pad, s);
                }
            }
        });

        ui.separator();
        ui.label(egui::RichText::new("Muestra bloqueada (pin)").color(T.accent).size(12.0));
        let pin_label = c.pin.as_deref().unwrap_or("(ninguna)").to_string();
        ui.label(egui::RichText::new(trunc(&pin_label, 28)).color(T.text_dim).size(10.0));
        if ui.button("Fijar la muestra actual").clicked() {
            // Toma la muestra del slot activo como pin
            let lib = lanzadera_engine::caja_drums::library_state::shared();
            let g = lib.lock().unwrap_or_else(|e| e.into_inner());
            let cell = g.lib.cells.get(pad).cloned().unwrap_or_default();
            if let Some(rel) = cell.get(slot) {
                kit.set_pin(pad, Some(rel.clone()));
            }
        }
        if c.pin.is_some() && ui.button("Desbloquear").clicked() {
            kit.set_pin(pad, None);
        }

        ui.separator();
        // ── Síntesis (pendiente de portar ParamCfg de Caja) ──
        ui.label(egui::RichText::new("Síntesis por celda").color(T.accent).size(12.0));
        for lbl in ["Pitch (semitonos)", "Decay (ms)", "Filtro (HPF/LPF)"] {
            ui.label(egui::RichText::new(format!("· {}", lbl)).color(T.text_dim).size(10.0));
        }
        ui.label(egui::RichText::new("→ se activan al portar ParamCfg de Caja")
            .color(T.text_dim).size(9.0));
    }
}

/// Paso actual del motor de Groove Machine (0..15, 255 = parado).
///
/// Doble fuente: `caja_sched.current_step` es el canal que publica el hilo
/// de audio del scheduler de Caja; `rt.groove_step()` queda como fallback
/// por compatibilidad. Así el playhead funciona sin depender de que el
/// audio escriba en RtStore.
fn groove_head(shared: &SessionShared) -> u8 {
    let c = shared.caja_sched.current_step.load(std::sync::atomic::Ordering::Relaxed);
    if c != 255 { c } else { shared.rt.groove_step() }
}

// ════════════════════════════════════════════════════════════════════════
//  Fase 4D — Modo CHORDS (Library): inspector de celda
//
//  Equivalencias con el MK3:
//    PITCH / MOD / PERFORM FX / NOTES = All roll · All lock · Todos 4c · Todos golpe
//    GROUP / AUTO / LOCK / NOTE REPEAT = locks de Muestra · Pitch · Decay · Filtro
// ════════════════════════════════════════════════════════════════════════
impl SessionView {
    fn inspector_groove_chords(&mut self, ui: &mut egui::Ui, shared: &Arc<SessionShared>) {
        use caja_core::kit::{is_lock, ParamCfg, P_DECAY, P_FILTER, P_PITCH, P_SAMPLE};
        use caja_core::pieces::{PAD_ORDER, PIECES};
        use lanzadera_engine::caja_drums::engine::{cutoff_hz, decay_seconds, pitch_semitones};

        let pad = self.groove_sel_pad.min(24);
        let kit = shared.kit.clone();
        let cfg = kit.snapshot_cfg();
        let c = cfg[pad].clone();
        let mode = kit.snapshot_live().get(pad).map(|l| l.mode).unwrap_or(0);
        let current = shared.chords_current_sample(pad);
        let bank = shared.chords_bank(pad);

        ui.heading("CHORDS · Biblioteca");
        ui.label(
            egui::RichText::new(format!("Pad {} · {}", pad + 1, PIECES[PAD_ORDER[pad]].name))
                .color(T.accent)
                .size(11.0),
        );
        ui.separator();

        // ── Acciones globales (PITCH / MOD / PERFORM / NOTES) ──
        ui.label(egui::RichText::new("Todas las celdas").color(T.accent).size(12.0));
        ui.horizontal_wrapped(|ui| {
            if ui.button("🎲 All roll").clicked() {
                shared.chords_all_roll();
                self.msg = "All roll: las celdas vuelven a tirar el dado".to_string();
            }
            if ui.button("🔒 All lock").clicked() {
                shared.chords_all_lock();
                self.msg = "All lock: kit congelado".to_string();
            }
            let rate_on = c.rate == 4;
            if ui
                .add(egui::Button::new("4c").fill(if rate_on { T.accent } else { T.panel }))
                .on_hover_text("Re-tirar el dado de todas las celdas cada 4 compases")
                .clicked()
            {
                let bars = shared.chords_toggle_rate(4);
                self.msg = if bars == 0 {
                    "4c: OFF".to_string()
                } else {
                    "4c: dado cada 4 compases".to_string()
                };
            }
            if ui.button("💥 Todos golpe").clicked() {
                for p in 0..lanzadera_engine::groove_kit::NUM_CELLS as u8 {
                    shared.apply_control(ControlCmd::LivePadOn { pad: p, vel: 100 });
                }
            }
        });

        ui.separator();

        // ── Locks de la celda (GROUP / AUTO / LOCK / NOTE REPEAT) ──
        ui.label(
            egui::RichText::new("Locks de la celda")
                .color(T.accent)
                .size(12.0),
        );
        for (idx, lbl) in [
            (P_SAMPLE, "Muestra"),
            (P_PITCH, "Pitch"),
            (P_DECAY, "Decay"),
            (P_FILTER, "Filtro"),
        ] {
            let mut on = is_lock(mode, idx);
            if ui.checkbox(&mut on, lbl).changed() {
                shared.chords_toggle_lock(pad, idx);
                // El encoder se asigna aunque el parámetro quede bloqueado.
                let target = if idx == P_SAMPLE { 255 } else { idx as u8 };
                shared.mk3_chords_param.store(target, std::sync::atomic::Ordering::Relaxed);
            }
        }
        ui.label(
            egui::RichText::new("En el MK3: LED brillante = bloqueado, débil = dado")
                .color(T.text_dim)
                .size(9.0),
        );

        ui.separator();

        // ── Muestra actual: dado, fijar, favorito ──
        ui.label(egui::RichText::new("Muestra").color(T.accent).size(12.0));
        let label = current
            .clone()
            .unwrap_or_else(|| "(sin muestras cargadas)".to_string());
        ui.label(egui::RichText::new(trunc(&label, 34)).color(T.text_dim).size(10.0))
            .on_hover_text(&label);
        let pinned = c.pin.clone();
        ui.horizontal_wrapped(|ui| {
            if ui.button("🎲 Dado").on_hover_text("Otra muestra de la celda").clicked() {
                shared.chords_roll(pad);
                shared.apply_control(ControlCmd::LivePadOn { pad: pad as u8, vel: 100 });
            }
            if ui.button("📌 Fijar").on_hover_text("Deja sonando esta muestra").clicked() {
                self.msg = match shared.chords_pin_current(pad) {
                    Some(rel) => format!("Fijada {}", rel),
                    None => "No hay muestra que fijar".to_string(),
                };
            }
            if pinned.is_some() && ui.button("✕ Quitar pin").clicked() {
                shared.chords_unpin(pad);
            }
            if let Some(rel) = current.clone() {
                let fav = shared.chords_is_favorite(&rel);
                let caption = if fav { "★ Quitar fav" } else { "☆ Favorito" };
                if ui.button(caption).clicked() {
                    shared.chords_toggle_favorite(&rel);
                }
            }
        });

        ui.separator();

        // ── Síntesis por celda: base ± rango + probabilidad ──
        ui.label(egui::RichText::new("Síntesis de la celda").color(T.accent).size(12.0));
        let mut param = c.param;
        let mut changes: Vec<(usize, ParamCfg)> = Vec::new();
        for (idx, lbl) in [
            (P_SAMPLE, "Muestra"),
            (P_PITCH, "Pitch"),
            (P_DECAY, "Decay"),
            (P_FILTER, "Filtro"),
        ] {
            let center = param[idx].base as f32 / 1000.0;
            let readout = match idx {
                P_SAMPLE => format!("recorre el {} % del banco", (param[idx].range / 10).max(1)),
                P_PITCH => format!("{:+.1} semitonos", pitch_semitones(center)),
                P_DECAY => match decay_seconds(center) {
                    Some(s) => format!("{:.0} ms", s * 1000.0),
                    None => "sin recorte".to_string(),
                },
                _ => match cutoff_hz(center) {
                    Some(h) => format!("{:.0} Hz", h),
                    None => "abierto".to_string(),
                },
            };
            ui.label(
                egui::RichText::new(format!("{} · {}", lbl, readout))
                    .color(T.text_dim)
                    .size(11.0),
            );
            let mut base = i32::from(param[idx].base);
            let mut range = i32::from(param[idx].range);
            let mut prob = i32::from(param[idx].prob);
            let mut dirty = false;
            ui.horizontal(|ui| {
                if ui.add(egui::Slider::new(&mut base, 0..=1000).text("base")).changed() {
                    dirty = true;
                }
                if ui.add(egui::Slider::new(&mut range, 0..=1000).text("rango")).changed() {
                    dirty = true;
                }
            });
            if ui.add(egui::Slider::new(&mut prob, 0..=1000).text("prob ‰")).changed() {
                dirty = true;
            }
            if dirty {
                param[idx] = ParamCfg {
                    base: base.clamp(0, 1000) as u16,
                    range: range.clamp(0, 1000) as u16,
                    prob: prob.clamp(0, 1000) as u16,
                };
                changes.push((idx, param[idx]));
            }
        }
        for (idx, pc) in changes {
            kit.set_param(pad, idx, pc);
        }

        ui.separator();

        // ── Banco de la celda (clic = fijar + escuchar) ──
        egui::CollapsingHeader::new(format!("Banco de la celda ({})", bank.len()))
            .default_open(false)
            .show(ui, |ui| {
                let mut pick: Option<String> = None;
                egui::ScrollArea::vertical()
                    .id_source("gm_chords_bank")
                    .max_height(180.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for rel in bank.iter().take(200) {
                            let name = std::path::Path::new(rel)
                                .file_name()
                                .map(|s| s.to_string_lossy().to_string())
                                .unwrap_or_else(|| rel.clone());
                            let is_cur = current.as_deref() == Some(rel.as_str());
                            let fill = if is_cur {
                                T.accent.gamma_multiply(0.35)
                            } else {
                                T.panel
                            };
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new(trunc(&name, 30)).size(10.0),
                                    )
                                    .fill(fill),
                                )
                                .on_hover_text(rel)
                                .clicked()
                            {
                                pick = Some(rel.clone());
                            }
                        }
                        if bank.is_empty() {
                            ui.label(
                                egui::RichText::new(
                                    "Sin muestras: revisa la carpeta en el Inspector del modo Pad",
                                )
                                .color(T.text_dim)
                                .size(10.0),
                            );
                        }
                    });
                if let Some(rel) = pick {
                    if shared.chords_pin(pad, &rel) {
                        shared.apply_control(ControlCmd::LivePadOn { pad: pad as u8, vel: 100 });
                        self.msg = format!("Fijada {}", rel);
                    } else {
                        self.msg = "Esa muestra ya no está en el banco".to_string();
                    }
                }
            });

        // ── Favoritos ──
        let favs = shared.chords_favorites();
        egui::CollapsingHeader::new(format!("⭐ Favoritos ({})", favs.len()))
            .default_open(false)
            .show(ui, |ui| {
                let mut remove: Option<String> = None;
                let mut assign: Option<String> = None;
                egui::ScrollArea::vertical()
                    .id_source("gm_chords_favs")
                    .max_height(160.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for rel in &favs {
                            let name = std::path::Path::new(rel)
                                .file_name()
                                .map(|s| s.to_string_lossy().to_string())
                                .unwrap_or_else(|| rel.clone());
                            ui.horizontal(|ui| {
                                if ui.small_button("📌").on_hover_text("Fijar en esta celda").clicked() {
                                    assign = Some(rel.clone());
                                }
                                ui.label(egui::RichText::new(trunc(&name, 24)).size(10.0))
                                    .on_hover_text(rel);
                                if ui.small_button("✕").clicked() {
                                    remove = Some(rel.clone());
                                }
                            });
                        }
                        if favs.is_empty() {
                            ui.label(
                                egui::RichText::new("Sin favoritos todavía")
                                    .color(T.text_dim)
                                    .size(10.0),
                            );
                        }
                    });
                if let Some(rel) = remove {
                    shared.chords_toggle_favorite(&rel);
                }
                if let Some(rel) = assign {
                    if shared.chords_pin(pad, &rel) {
                        shared.apply_control(ControlCmd::LivePadOn { pad: pad as u8, vel: 100 });
                        self.msg = format!("Fijada {}", rel);
                    } else {
                        self.msg = "Ese favorito no está en el banco de esta celda".to_string();
                    }
                }
            });
    }
}

/// Etiqueta legible de una FollowAction. `JumpTo` muestra la escena 1-based.
fn follow_label(f: FollowAction) -> String {
    match f {
        FollowAction::JumpTo(n) => format!("Salta a {}", n as usize + 1),
        other => format!("{:?}", other),
    }
}
