// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
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
use crate::plugin_window::PluginWindows;

#[derive(Clone, Copy, PartialEq)]
pub enum BrowserMode { Clap, Vst3 }
/// A qué ranura de la pista va el plugin que se elige en el browser.
#[derive(Clone, Copy, PartialEq)]
pub enum BrowserTarget { Instrument, NoteFx, Fx }
/// Pestaña del browser global integrado.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BrowserTab { Files, Instruments, NoteFx, Fx }
/// Pestañas de la ventana de Preferencias.
#[derive(Clone, Copy, PartialEq)]
pub enum PrefsTab { Plugins, Midi, Audio, Apariencia }
/// Resultado de un escaneo de plugins en segundo plano.
type ScanResult = (BrowserMode, Vec<(String, String, String)>, std::collections::HashMap<(String, String), lanzadera_core::PluginKind>);
/// Qué se pidió en un diálogo de archivo / de ruta.
#[derive(Clone, Copy, PartialEq)]
pub enum DialogKind { Open, Save }
/// Acción que espera a que el usuario decida qué hacer con los cambios sin guardar.
#[derive(Clone, PartialEq)]
pub enum PendingAction { New, Open, OpenPath(String), Quit }
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
    pub clip_launch_pos: Option<egui::Pos2>,
    pub groove_pos: Option<egui::Pos2>,
    pub inspector_pos: Option<egui::Pos2>,
    pub browser_pos: Option<egui::Pos2>,
    pub mixer_pos: Option<egui::Pos2>,
    pub clip_launch_size: Option<egui::Vec2>,
    pub groove_size: Option<egui::Vec2>,
    pub inspector_size: Option<egui::Vec2>,
    pub browser_size: Option<egui::Vec2>,
    pub mixer_size: Option<egui::Vec2>,
}

impl Default for WindowState {
    fn default() -> Self {
        WindowState {
            clip_launch_visible: true,
            groove_machine_visible: false,
            inspector_visible: true,
            browser_visible: false,
            mixer_visible: false,
            clip_launch_pos: None,
            groove_pos: None,
            inspector_pos: None,
            browser_pos: None,
            mixer_pos: None,
            clip_launch_size: None,
            groove_size: None,
            inspector_size: None,
            browser_size: None,
            mixer_size: None,
        }
    }
}


#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrooveScreenTab {
    Main,
    Step,
}

impl Default for GrooveScreenTab {
    fn default() -> Self {
        GrooveScreenTab::Main
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrooveDetailTab {
    Chord,
    Params,
}

impl Default for GrooveDetailTab {
    fn default() -> Self {
        GrooveDetailTab::Chord
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrooveMainFocus {
    Pads,
    Detail,
    Rack,
}

impl Default for GrooveMainFocus {
    fn default() -> Self {
        GrooveMainFocus::Pads
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

/// Portapapeles de clip para copiar/pegar/cortar en el grid.
#[derive(Clone)]
pub struct ClipClipboard {
    pub clip: lanzadera_core::Clip,
    pub midi: Option<std::sync::Arc<lanzadera_engine::midi::MidiNoteData>>,
    pub sample: Option<std::sync::Arc<lanzadera_engine::shared::SampleData>>,
}

pub struct SessionView {
    pub windows: WindowState,
    pub groove_tab: GrooveScreenTab,
    pub groove_detail_tab: GrooveDetailTab,
    pub groove_main_focus: GrooveMainFocus,
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
    /// Cache de overview de waveform por pad para la GUI (UI-only).
    pub groove_wave_cache_pad: [usize; 25],
    pub groove_wave_cache: [Option<std::sync::Arc<Vec<f32>>>; 25],
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
    pub groove_sample_zoom: f32,
    pub groove_sample_scroll: f32,
    pub groove_sample_sel: Option<(f32, f32)>,
    pub groove_sample_gain_db: f32,
    pub groove_sample_msg: String,
    pub groove_sample_pad: usize,
    pub groove_sample_slot: usize,
    pub inspector_focus: InspectorFocus,
    pub browser_path: std::path::PathBuf,
    pub browser_filter: String,
    pub browser_favorites_only: bool,
    pub msg: String,
    pub project_path: String,
    /// Proyectos recientes (más nuevo primero, máx. 10). Se guardan en ~/.config/lanzadera/recent.txt
    pub recent_projects: Vec<String>,
    /// Diálogo de archivo nativo en curso (corre en un hilo aparte para no congelar la UI).
    pub dialog_rx: Option<std::sync::mpsc::Receiver<(DialogKind, Option<std::path::PathBuf>)>>,
    /// Alternativa por ruta escrita, por si el sistema no tiene diálogos nativos.
    pub path_prompt: Option<(DialogKind, String)>,
    pub show_shortcuts: bool,
    pub show_about: bool,
    pub last_title: String,
    // ── Fase 2: cambios sin guardar, autoguardado y recuperación ──
    pub now: f64,
    pub dirty: bool,
    pub force_dirty: bool,
    pub saved_fp: Option<u64>,
    pub baseline_at: Option<f64>,
    pub last_fp_check: f64,
    pub last_autosave: f64,
    pub confirm: Option<PendingAction>,
    pub after_save: Option<PendingAction>,
    pub force_close: bool,
    pub recover_prompt: Option<String>,
    pub startup_checked: bool,
    // ── Fase 2b: guardado en dos tiempos (captura de plugins → escritura) ──
    pub save_pending: bool,
    // ── Fase 3: preferencias y escaneo en segundo plano ──
    pub prefs_open: bool,
    pub prefs_tab: PrefsTab,
    pub folder_rx: Option<std::sync::mpsc::Receiver<(BrowserMode, Option<std::path::PathBuf>)>>,
    pub scan_rx: Option<std::sync::mpsc::Receiver<ScanResult>>,
    pub midi_last_poll: f64,
    pub ui_scale_edit: f32,
    pub autosave_pending: bool,
    pub ctx_cache: Option<egui::Context>,
    pub audio_modal: Option<(usize, usize)>,
    pub audio_path_input: String,
    pub roll: PianoRoll,
    pub sample_editor: crate::sample_editor::SampleEditor,
    pub plugin_path_input: String,
    pub browser_open: bool,
    pub browser_tab: BrowserTab,
    pub browser_plugin_filter: String,
    pub browser_list: Option<Vec<(String, String, String)>>,
    pub browser_mode: BrowserMode,
    pub browser_target: BrowserTarget,
    pub browser_kinds: std::collections::HashMap<(String, String), lanzadera_core::PluginKind>,
    pub browser_show_all: bool,
    pub plugin_windows: PluginWindows,
    pub pending_native_gui: Vec<usize>,
    pub groove_sequencer_open: bool,
    /// Portapapeles interno de clips (copiar/pegar/cortar).
    pub clip_clipboard: Option<ClipClipboard>,
    /// Frames restantes para forzar posición de ventanas tras cargar layout.
    pub window_layout_apply_frames: u8,
}

impl Default for SessionView {
    fn default() -> Self {
        SessionView {
            windows: WindowState::default(),
            groove_tab: GrooveScreenTab::default(),
            groove_detail_tab: GrooveDetailTab::default(),
            groove_main_focus: GrooveMainFocus::default(),
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
            groove_wave_cache_pad: [usize::MAX; 25],
            groove_wave_cache: std::array::from_fn(|_| None),
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
            groove_sample_zoom: 1.0,
            groove_sample_scroll: 0.0,
            groove_sample_sel: None,
            groove_sample_gain_db: 0.0,
            groove_sample_msg: String::new(),
            groove_sample_pad: usize::MAX,
            groove_sample_slot: usize::MAX,
            inspector_focus: InspectorFocus::default(),
            browser_path: std::path::PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into())),
            browser_filter: String::new(),
            browser_favorites_only: false,
            msg: String::new(),
            project_path: "proyecto.lanzadera".into(),
            recent_projects: Self::load_recent_projects(),
            dialog_rx: None,
            path_prompt: None,
            show_shortcuts: false,
            show_about: false,
            last_title: String::new(),
            now: 0.0,
            dirty: false,
            force_dirty: false,
            saved_fp: None,
            baseline_at: None,
            last_fp_check: 0.0,
            last_autosave: 0.0,
            confirm: None,
            after_save: None,
            force_close: false,
            recover_prompt: None,
            startup_checked: false,
            save_pending: false,
            prefs_open: false,
            prefs_tab: PrefsTab::Plugins,
            folder_rx: None,
            scan_rx: None,
            midi_last_poll: 0.0,
            ui_scale_edit: 1.0,
            autosave_pending: false,
            ctx_cache: None,
            audio_modal: None,
            audio_path_input: String::new(),
            roll: PianoRoll::default(),
            sample_editor: crate::sample_editor::SampleEditor::default(),
            plugin_path_input: String::new(),
            browser_open: false,
            browser_tab: BrowserTab::Files,
            browser_plugin_filter: String::new(),
            browser_list: None,
            browser_mode: BrowserMode::Clap,
            browser_target: BrowserTarget::Instrument,
            browser_kinds: std::collections::HashMap::new(),
            browser_show_all: false,
            plugin_windows: PluginWindows::default(),
            pending_native_gui: Vec::new(),
            groove_sequencer_open: false,
            clip_clipboard: None,
            window_layout_apply_frames: 0,
        }
    }
}

impl SessionView {
    pub fn take_native_gui_requests(&mut self) -> Vec<usize> {
        std::mem::take(&mut self.pending_native_gui)
    }

    pub fn show(&mut self, ctx: &egui::Context, shared: &Arc<SessionShared>, audio_on: bool) {
        // ── Fase 1.4C.2b: peticiones del MK3 físico ──────────────────
        // Pestaña/foco de Groove Machine pedido por PAD_MODE/STEP/KEYBOARD/CHORDS
        let mode_req = shared.mk3_mode_req.swap(255, std::sync::atomic::Ordering::Relaxed);
        if mode_req != 255 {
            self.groove_chords = mode_req == 3;
            match mode_req {
                1 => {
                    self.groove_tab = GrooveScreenTab::Step;
                    self.groove_main_focus = GrooveMainFocus::Rack;
                }
                2 => {
                    self.groove_tab = GrooveScreenTab::Main;
                    self.groove_detail_tab = GrooveDetailTab::Params;
                    self.groove_main_focus = GrooveMainFocus::Detail;
                }
                3 => {
                    self.groove_tab = GrooveScreenTab::Main;
                    self.groove_detail_tab = GrooveDetailTab::Chord;
                    self.groove_main_focus = GrooveMainFocus::Detail;
                }
                _ => {
                    self.groove_tab = GrooveScreenTab::Main;
                    // Si EVENTS está activo, mantener foco en la tira inferior.
                    self.groove_main_focus = if self.groove_event { GrooveMainFocus::Rack } else { GrooveMainFocus::Pads };
                }
            }
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
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("clip_clipboard_has"), self.clip_clipboard.is_some()));
        self.roll.show(ctx, shared);
        if self.windows.clip_launch_visible {
            self.clip_launch_window(ctx, shared, audio_on);
        }

        // Groove Machine: Pad / Mixer / Step.
        if self.windows.groove_machine_visible {
            self.groove_machine_window(ctx, shared);
        }

        // Sub-modo publicado para MK3/LEDs. Se mantiene la semántica lógica
        // aunque la UI muestre una pantalla unificada: 0=Pad 1=Step 2=Mixer 3=Chords.
        let sub_mode = if self.groove_chords {
            3
        } else {
            match self.groove_tab {
                GrooveScreenTab::Step => 1,
                GrooveScreenTab::Main => match self.groove_main_focus {
                    GrooveMainFocus::Detail if self.groove_detail_tab == GrooveDetailTab::Params => 2,
                    _ => 0,
                },
            }
        };
        shared.groove_sub_mode.store(sub_mode, std::sync::atomic::Ordering::Relaxed);
        shared.mk3_chords_mode.store(self.groove_chords, std::sync::atomic::Ordering::Relaxed);

        // Browser global integrado (archivos + plugins).
        if self.windows.browser_visible {
            self.global_browser_window(ctx, shared);
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
        self.sample_editor.show(ctx, shared);
        // ── Fase 1: menús, atajos y diálogos de proyecto ──
        self.project_tick(ctx, shared);
        self.poll_scan();
        self.poll_folder_dialog();
        self.prefs_window(ctx);
        self.confirm_window(ctx, shared);
        self.recover_window(ctx, shared);
        self.handle_shortcuts(ctx, shared);
        self.poll_dialog(shared);
        self.path_prompt_window(ctx, shared);
        self.help_windows(ctx);
        // ── Ventanas flotantes de plugins (Fase 3H) ──
        let native_reqs = self.plugin_windows.show(ctx, shared);
        self.pending_native_gui.extend(native_reqs);
        self.window_layout_apply_frames = self.window_layout_apply_frames.saturating_sub(1);
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }

    // ── Menús, proyectos y diálogos de archivo (Fase 1) ────────────────────
    fn config_dir() -> Option<std::path::PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))?;
        Some(base.join("lanzadera"))
    }

    pub fn load_recent_projects() -> Vec<String> {
        let Some(dir) = Self::config_dir() else { return Vec::new() };
        std::fs::read_to_string(dir.join("recent.txt")).ok()
            .map(|s| s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).take(10).collect())
            .unwrap_or_default()
    }

    fn push_recent(&mut self, path: &str) {
        self.recent_projects.retain(|p| p != path);
        self.recent_projects.insert(0, path.to_string());
        self.recent_projects.truncate(10);
        if let Some(dir) = Self::config_dir() {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join("recent.txt"), self.recent_projects.join("\n"));
        }
    }

    fn project_name(&self) -> String {
        std::path::Path::new(&self.project_path).file_stem()
            .and_then(|s| s.to_str()).unwrap_or("sin título").to_string()
    }

    /// Guardar en dos tiempos: primero se pide al hilo principal el estado de los plugins
    /// (es quien los posee) y, cuando está listo, `project_tick` escribe el archivo.
    fn project_save(&mut self, _shared: &SessionShared) {
        lanzadera_engine::state_ctl::request_capture();
        self.save_pending = true;
        if let Some(c) = &self.ctx_cache { c.request_repaint(); }
    }

    /// Escritura real del proyecto. Devuelve true si se guardó bien.
    fn project_save_now(&mut self, shared: &SessionShared) -> bool {
        self.sync_window_layout_to_state(shared);
        let path = self.project_path.clone();
        match shared.save_to_file(&path) {
            Ok(()) => {
                self.push_recent(&path);
                self.force_dirty = false;
                self.dirty = false;
                self.baseline_at = None;
                self.saved_fp = Some(shared.project_fingerprint());
                Self::clear_autosave();
                self.msg = format!("Guardado {}", path);
                true
            }
            Err(e) => { self.msg = format!("⚠ No se pudo guardar: {}", e); false }
        }
    }

    fn write_autosave(&mut self, shared: &SessionShared) {
        self.sync_window_layout_to_state(shared);
        if let Some((a, o)) = Self::autosave_paths() {
            if let Some(dir) = a.parent() { let _ = std::fs::create_dir_all(dir); }
            if shared.autosave_to_file(&a.to_string_lossy()).is_ok() {
                let _ = std::fs::write(&o, &self.project_path);
            }
        }
    }

    fn project_open(&mut self, shared: &SessionShared, path: String) {
        self.msg = match shared.load_from_file(&path) {
            Ok(()) => {
                self.project_path = path.clone();
                self.push_recent(&path);
                self.window_layout_apply_frames = 3;
                self.apply_window_layout_from_state(shared);
                // Reinicia el audio para cargar los plugins (instrumento, Note FX, FX) del proyecto.
                lanzadera_engine::state_ctl::skip_capture_on_next_restart();
                shared.audio_restart.store(true, std::sync::atomic::Ordering::Relaxed);
                self.reset_dirty_baseline();
                format!("Abierto {}", path)
            }
            Err(e) => format!("⚠ No se pudo abrir: {}", e),
        };
    }

    fn project_new(&mut self, shared: &SessionShared) {
        shared.new_project();
        if let Some(tpl) = lanzadera_core::config::get().window_layout_template {
            shared.edit(|st| st.window_layout = tpl);
        }
        self.window_layout_apply_frames = 3;
        self.apply_window_layout_from_state(shared);
        lanzadera_engine::state_ctl::skip_capture_on_next_restart();
        shared.audio_restart.store(true, std::sync::atomic::Ordering::Relaxed);
        self.project_path = "sin_titulo.lanzadera".into();
        self.reset_dirty_baseline();
        self.msg = "Proyecto nuevo".into();
    }

    // ── Fase 2: cambios sin guardar, confirmaciones, autoguardado ──────────
    fn autosave_paths() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
        let dir = Self::config_dir()?.join("autosave");
        Some((dir.join("autosave.lanzadera"), dir.join("autosave.origin")))
    }

    fn apply_window_layout_from_state(&mut self, shared: &SessionShared) {
        let wl = { shared.state.lock().unwrap_or_else(|e| e.into_inner()).window_layout.clone() };
        self.windows.clip_launch_visible = wl.clip_launch;
        self.windows.groove_machine_visible = wl.groove_machine;
        self.windows.inspector_visible = wl.inspector;
        self.windows.browser_visible = wl.browser;
        self.windows.mixer_visible = wl.mixer;
        self.windows.clip_launch_pos = wl.clip_launch_pos.map(|p| egui::pos2(p[0], p[1]));
        self.windows.groove_pos = wl.groove_pos.map(|p| egui::pos2(p[0], p[1]));
        self.windows.inspector_pos = wl.inspector_pos.map(|p| egui::pos2(p[0], p[1]));
        self.windows.browser_pos = wl.browser_pos.map(|p| egui::pos2(p[0], p[1]));
        self.windows.mixer_pos = wl.mixer_pos.map(|p| egui::pos2(p[0], p[1]));
        self.windows.clip_launch_size = wl.clip_launch_size.map(|s| egui::vec2(s[0], s[1]));
        self.windows.groove_size = wl.groove_size.map(|s| egui::vec2(s[0], s[1]));
        self.windows.inspector_size = wl.inspector_size.map(|s| egui::vec2(s[0], s[1]));
        self.windows.browser_size = wl.browser_size.map(|s| egui::vec2(s[0], s[1]));
        self.windows.mixer_size = wl.mixer_size.map(|s| egui::vec2(s[0], s[1]));
        self.window_layout_apply_frames = 3;
    }

    fn sync_window_layout_to_state(&self, shared: &SessionShared) {
        let wl = lanzadera_core::session::WindowLayoutState {
            clip_launch: self.windows.clip_launch_visible,
            groove_machine: self.windows.groove_machine_visible,
            inspector: self.windows.inspector_visible,
            browser: self.windows.browser_visible,
            mixer: self.windows.mixer_visible,
            clip_launch_pos: self.windows.clip_launch_pos.map(|p| [p.x, p.y]),
            groove_pos: self.windows.groove_pos.map(|p| [p.x, p.y]),
            inspector_pos: self.windows.inspector_pos.map(|p| [p.x, p.y]),
            browser_pos: self.windows.browser_pos.map(|p| [p.x, p.y]),
            mixer_pos: self.windows.mixer_pos.map(|p| [p.x, p.y]),
            clip_launch_size: self.windows.clip_launch_size.map(|s| [s.x, s.y]),
            groove_size: self.windows.groove_size.map(|s| [s.x, s.y]),
            inspector_size: self.windows.inspector_size.map(|s| [s.x, s.y]),
            browser_size: self.windows.browser_size.map(|s| [s.x, s.y]),
            mixer_size: self.windows.mixer_size.map(|s| [s.x, s.y]),
        };
        shared.edit(|st| st.window_layout = wl);
    }

    fn clear_autosave() {
        if let Some((a, o)) = Self::autosave_paths() {
            let _ = std::fs::remove_file(a);
            let _ = std::fs::remove_file(o);
        }
    }

    /// Tras abrir o crear un proyecto: la «huella limpia» se toma 2 s después, cuando
    /// ya terminaron las cargas asíncronas de audio y plugins.
    fn reset_dirty_baseline(&mut self) {
        self.force_dirty = false;
        self.dirty = false;
        self.saved_fp = None;
        self.baseline_at = Some(self.now + 2.0);
    }

    fn needs_save_as(&self) -> bool {
        !std::path::Path::new(&self.project_path).exists()
    }

    fn project_save_or_ask(&mut self, shared: &SessionShared) {
        if self.needs_save_as() { self.spawn_file_dialog(DialogKind::Save); }
        else { self.project_save(shared); }
    }

    fn run_action(&mut self, shared: &SessionShared, a: PendingAction) {
        match a {
            PendingAction::New => self.project_new(shared),
            PendingAction::Open => self.spawn_file_dialog(DialogKind::Open),
            PendingAction::OpenPath(p) => self.project_open(shared, p),
            PendingAction::Quit => { Self::clear_autosave(); self.force_close = true; }
        }
    }

    /// Ejecuta la acción, o pregunta primero si hay cambios sin guardar.
    fn request_action(&mut self, shared: &SessionShared, a: PendingAction) {
        if self.dirty { self.confirm = Some(a); } else { self.run_action(shared, a); }
    }

    /// Una vez por frame: huella (1 Hz), autoguardado (60 s), arranque y cierre de ventana.
    fn project_tick(&mut self, ctx: &egui::Context, shared: &SessionShared) {
        self.now = ctx.input(|i| i.time);
        let now = self.now;
        self.ctx_cache = Some(ctx.clone());

        // Guardado en dos tiempos: cuando el hilo principal ya capturó los plugins, se escribe.
        if self.save_pending && !lanzadera_engine::state_ctl::capture_pending() {
            self.save_pending = false;
            let ok = self.project_save_now(shared);
            // Nuevo / Abrir / Salir solo siguen si el guardado salió bien.
            if let Some(a) = self.after_save.take() {
                if ok { self.run_action(shared, a); }
            }
        }
        if self.autosave_pending && !lanzadera_engine::state_ctl::capture_pending() {
            self.autosave_pending = false;
            if self.recover_prompt.is_none() { self.write_autosave(shared); }
        }
        if self.save_pending || self.autosave_pending { ctx.request_repaint(); }
        // Un plugin CLAP avisó de que su estado cambió: el proyecto tiene cambios.
        if lanzadera_engine::state_ctl::take_plugin_dirty() && self.baseline_at.is_none() && self.saved_fp.is_some() {
            self.force_dirty = true;
            self.dirty = true;
        }

        if !self.startup_checked {
            self.startup_checked = true;
            let sc = lanzadera_core::config::get().ui_scale;
            self.ui_scale_edit = sc;
            if (sc - 1.0).abs() > 0.01 { ctx.set_zoom_factor(sc); }
            if let Some((a, o)) = Self::autosave_paths() {
                if a.exists() {
                    self.recover_prompt = Some(std::fs::read_to_string(&o).unwrap_or_default().trim().to_string());
                }
            }
            self.baseline_at = Some(now + 2.0);
        }
        if let Some(t) = self.baseline_at {
            if now >= t {
                self.saved_fp = Some(shared.project_fingerprint());
                self.baseline_at = None;
            }
        }
        if now - self.last_fp_check >= 1.0 {
            self.last_fp_check = now;
            if self.baseline_at.is_none() {
                if let Some(b) = self.saved_fp {
                    self.dirty = self.force_dirty || shared.project_fingerprint() != b;
                }
            }
            // Autoguardado: nunca mientras se decide si recuperar el anterior.
            if self.dirty && self.recover_prompt.is_none() && !self.autosave_pending
                && now - self.last_autosave >= 60.0 {
                self.last_autosave = now;
                lanzadera_engine::state_ctl::request_capture();
                self.autosave_pending = true;
            }
        }
        ctx.request_repaint_after(std::time::Duration::from_secs(1));

        // Cierre de la ventana: si hay cambios, se pregunta antes.
        if self.force_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if ctx.input(|i| i.viewport().close_requested()) && self.dirty {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.confirm.is_none() { self.confirm = Some(PendingAction::Quit); }
        }
    }

    fn confirm_window(&mut self, ctx: &egui::Context, shared: &SessionShared) {
        let Some(action) = self.confirm.clone() else { return };
        let mut choice: Option<u8> = None;
        egui::Window::new("Cambios sin guardar").collapsible(false).resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                ui.label(format!("«{}» tiene cambios sin guardar.", self.project_name()));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Guardar").clicked() { choice = Some(0); }
                    if ui.button("No guardar").clicked() { choice = Some(1); }
                    if ui.button("Cancelar").clicked() { choice = Some(2); }
                });
            });
        match choice {
            Some(0) => {
                self.confirm = None;
                // La acción (Nuevo / Abrir / Salir) sigue solo cuando el guardado termina bien.
                self.after_save = Some(action);
                if self.needs_save_as() {
                    self.spawn_file_dialog(DialogKind::Save);
                } else {
                    self.project_save(shared);
                }
            }
            Some(1) => { self.confirm = None; self.run_action(shared, action); }
            Some(2) => { self.confirm = None; }
            _ => {}
        }
    }

    fn recover_window(&mut self, ctx: &egui::Context, shared: &SessionShared) {
        let Some(origin) = self.recover_prompt.clone() else { return };
        let name = std::path::Path::new(&origin).file_stem().and_then(|s| s.to_str())
            .filter(|s| !s.is_empty()).unwrap_or("proyecto sin nombre").to_string();
        let (mut recover, mut discard) = (false, false);
        egui::Window::new("Autoguardado encontrado").collapsible(false).resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                ui.label(format!("Lanzadera se cerró sin guardar «{}».", name));
                ui.label("¿Quieres recuperar el trabajo autoguardado?");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Recuperar").clicked() { recover = true; }
                    if ui.button("Descartar").clicked() { discard = true; }
                });
            });
        if recover {
            self.recover_prompt = None;
            if let Some((a, _)) = Self::autosave_paths() {
                self.msg = match shared.load_from_file(&a.to_string_lossy()) {
                    Ok(()) => {
                        lanzadera_engine::state_ctl::skip_capture_on_next_restart();
                        shared.audio_restart.store(true, std::sync::atomic::Ordering::Relaxed);
                        if !origin.is_empty() { self.project_path = origin; }
                        self.reset_dirty_baseline();
                        // Lo recuperado no está guardado en el proyecto real.
                        self.force_dirty = true;
                        self.dirty = true;
                        "Autoguardado recuperado: guarda el proyecto para conservarlo".to_string()
                    }
                    Err(e) => format!("⚠ No se pudo recuperar el autoguardado: {}", e),
                };
            }
        } else if discard {
            self.recover_prompt = None;
            Self::clear_autosave();
        }
    }

    /// Abre el diálogo nativo de archivos en un hilo aparte; el resultado se recoge en `poll_dialog`.
    fn spawn_file_dialog(&mut self, kind: DialogKind) {
        if self.dialog_rx.is_some() { return; } // ya hay uno abierto
        let (tx, rx) = std::sync::mpsc::channel();
        self.dialog_rx = Some(rx);
        let start = std::path::Path::new(&self.project_path).parent()
            .map(|p| p.to_path_buf()).filter(|p| p.is_dir());
        let name = std::path::Path::new(&self.project_path).file_name()
            .and_then(|n| n.to_str()).unwrap_or("proyecto.lanzadera").to_string();
        std::thread::spawn(move || {
            let mut d = rfd::FileDialog::new().add_filter("Proyecto Lanzadera", &["lanzadera"]);
            if let Some(dir) = start { d = d.set_directory(dir); }
            let res = match kind {
                DialogKind::Open => d.pick_file(),
                DialogKind::Save => d.set_file_name(name).save_file(),
            };
            let _ = tx.send((kind, res));
        });
    }

    fn poll_dialog(&mut self, shared: &SessionShared) {
        let Some(rx) = &self.dialog_rx else { return };
        match rx.try_recv() {
            Ok((kind, Some(mut path))) => {
                self.dialog_rx = None;
                match kind {
                    DialogKind::Open => self.project_open(shared, path.to_string_lossy().into_owned()),
                    DialogKind::Save => {
                        if path.extension().is_none() { path.set_extension("lanzadera"); }
                        self.project_path = path.to_string_lossy().into_owned();
                        self.project_save(shared);
                    }
                }
            }
            // Cancelado, o el sistema no tiene portal de diálogos: queda la opción «por ruta».
            Ok((_, None)) => { self.dialog_rx = None; self.after_save = None; }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => { self.dialog_rx = None; self.after_save = None; }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }

    fn path_prompt_window(&mut self, ctx: &egui::Context, shared: &SessionShared) {
        let Some((kind, mut text)) = self.path_prompt.take() else { return };
        let mut keep = true;
        let mut done: Option<String> = None;
        let title = if kind == DialogKind::Open { "Abrir proyecto por ruta" } else { "Guardar proyecto como (ruta)" };
        egui::Window::new(title).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.add(egui::TextEdit::singleline(&mut text).desired_width(380.0));
            ui.horizontal(|ui| {
                if ui.button("Aceptar").clicked() { done = Some(text.clone()); }
                if ui.button("Cancelar").clicked() { keep = false; }
            });
        });
        if let Some(p) = done {
            match kind {
                DialogKind::Open => self.request_action(shared, PendingAction::OpenPath(p)),
                DialogKind::Save => { self.project_path = p; self.project_save(shared); }
            }
        } else if keep {
            self.path_prompt = Some((kind, text));
        }
    }

    /// Atajos globales estilo DAW. Se ignoran si hay un campo de texto con foco.
    fn handle_shortcuts(&mut self, ctx: &egui::Context, shared: &SessionShared) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let wants_text = ctx.wants_keyboard_input();
        let roll_open = self.roll.open;
        let (save_as, save, open, new, copy, paste, cut, dup, del) = ctx.input_mut(|i| {
            let sa = i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::S));
            let s = i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::S));
            let o = i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::O));
            let n = i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::N));
            let clip_shortcuts = !wants_text && !roll_open;
            let copy = clip_shortcuts && i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::C));
            let paste = clip_shortcuts && i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::V));
            let cut = clip_shortcuts && i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::X));
            let dup = clip_shortcuts && i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::D));
            let del = clip_shortcuts && (i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace));
            (sa, s, o, n, copy, paste, cut, dup, del)
        });
        if new { self.request_action(shared, PendingAction::New); }
        if open { self.request_action(shared, PendingAction::Open); }
        if save_as { self.spawn_file_dialog(DialogKind::Save); }
        else if save { self.project_save_or_ask(shared); }

        if copy { self.copy_selected_clip(shared); }
        if cut { self.cut_selected_clip(shared); }
        if paste { self.paste_clip(shared); }
        if dup { self.duplicate_selected_clip(shared); }
        if del { self.delete_selected_clip(shared); }

        let title = format!("{}{} — Lanzadera", self.project_name(), if self.dirty { " *" } else { "" });
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
    }

    fn selected_clip_slot(&self, shared: &SessionShared) -> Option<(usize, usize)> {
        shared.state.lock().unwrap_or_else(|e| e.into_inner()).selected_clip
    }

    fn copy_selected_clip(&mut self, shared: &SessionShared) {
        let Some((t, s)) = self.selected_clip_slot(shared) else { return; };
        let clip = {
            let st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            st.tracks.get(t).and_then(|tr| tr.clips.get(s)).and_then(|c| c.clone())
        };
        let Some(clip) = clip else { self.msg = "No hay clip para copiar".into(); return; };
        self.clip_clipboard = Some(ClipClipboard {
            clip,
            midi: shared.notes.load(t, s),
            sample: shared.samples.load(t, s),
        });
        self.msg = "Clip copiado".into();
    }

    fn cut_selected_clip(&mut self, shared: &SessionShared) {
        let Some((t, s)) = self.selected_clip_slot(shared) else { return; };
        let has = { shared.state.lock().unwrap_or_else(|e| e.into_inner())
            .tracks.get(t).and_then(|tr| tr.clips.get(s)).map(|c| c.is_some()).unwrap_or(false) };
        if !has { self.msg = "No hay clip para cortar".into(); return; }
        self.copy_selected_clip(shared);
        self.delete_clip_at(shared, t, s);
        self.msg = "Clip cortado".into();
    }

    fn paste_clip(&mut self, shared: &SessionShared) {
        let Some(cb) = self.clip_clipboard.clone() else { self.msg = "Portapapeles vacío".into(); return; };
        let (t, s) = {
            let st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            st.selected_clip.unwrap_or((st.selected_track, st.selected_scene))
        };
        self.write_clip_to_slot(shared, t, s, cb.clip, cb.midi, cb.sample, true);
        self.msg = format!("Clip pegado en P{}·E{}", t + 1, s + 1);
    }

    fn duplicate_selected_clip(&mut self, shared: &SessionShared) {
        let Some((t, s)) = self.selected_clip_slot(shared) else { return; };
        let (clip, midi, sample) = {
            let st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            let clip = st.tracks.get(t).and_then(|tr| tr.clips.get(s)).and_then(|c| c.clone());
            (clip, shared.notes.load(t, s), shared.samples.load(t, s))
        };
        let Some(clip) = clip else { self.msg = "No hay clip para duplicar".into(); return; };
        let target = shared.edit(|st| {
            st.tracks.get_mut(t).and_then(|tr| tr.clips.iter().position(|x| x.is_none()))
        });
        let Some(ds) = target else { self.msg = "No hay escena vacía en la pista".into(); return; };
        self.write_clip_to_slot(shared, t, ds, clip, midi, sample, true);
        shared.apply_control(ControlCmd::SelectClip { track: t as u8, scene: ds as u8 });
        self.msg = format!("Clip duplicado en P{}·E{}", t + 1, ds + 1);
    }

    fn delete_selected_clip(&mut self, shared: &SessionShared) {
        let Some((t, s)) = self.selected_clip_slot(shared) else { return; };
        let has = { shared.state.lock().unwrap_or_else(|e| e.into_inner())
            .tracks.get(t).and_then(|tr| tr.clips.get(s)).map(|c| c.is_some()).unwrap_or(false) };
        if !has { return; }
        self.delete_clip_at(shared, t, s);
        self.msg = "Clip eliminado".into();
    }

    fn delete_clip_at(&self, shared: &SessionShared, t: usize, s: usize) {
        shared.apply_control(ControlCmd::StopClip { track: t as u8 });
        shared.edit(|st| {
            if let Some(tr) = st.tracks.get_mut(t) {
                if let Some(slot) = tr.clips.get_mut(s) { *slot = None; }
            }
            if st.selected_clip == Some((t, s)) { st.selected_clip = None; }
        });
        shared.sync_midi(t, s);
        shared.samples.clear(t, s);
    }

    fn write_clip_to_slot(
        &self, shared: &SessionShared, t: usize, s: usize, mut clip: lanzadera_core::Clip,
        midi: Option<std::sync::Arc<lanzadera_engine::midi::MidiNoteData>>,
        sample: Option<std::sync::Arc<lanzadera_engine::shared::SampleData>>,
        stop_target: bool,
    ) {
        if stop_target { shared.apply_control(ControlCmd::StopClip { track: t as u8 }); }
        clip.state = lanzadera_core::ClipState::Empty;
        shared.edit(|st| {
            if let Some(tr) = st.tracks.get_mut(t) {
                if let Some(slot) = tr.clips.get_mut(s) { *slot = Some(clip); }
            }
            st.selected_track = t; st.selected_scene = s; st.selected_clip = Some((t, s));
        });
        if let Some(m) = midi { shared.notes.store(t, s, m); } else { shared.notes.clear(t, s); }
        if let Some(sd) = sample { shared.samples.store(t, s, sd); } else { shared.samples.clear(t, s); }
        shared.sync_midi(t, s);
    }

    fn move_or_copy_clip(&mut self, shared: &SessionShared, st_t: usize, st_s: usize, dt: usize, ds: usize, copy: bool) {
        if st_t == dt && st_s == ds { return; }
        let (clip, midi, sample) = {
            let st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            let clip = st.tracks.get(st_t).and_then(|tr| tr.clips.get(st_s)).and_then(|c| c.clone());
            (clip, shared.notes.load(st_t, st_s), shared.samples.load(st_t, st_s))
        };
        let Some(clip) = clip else { return; };
        self.write_clip_to_slot(shared, dt, ds, clip, midi, sample, true);
        if !copy {
            self.delete_clip_at(shared, st_t, st_s);
            self.msg = format!("Clip movido a P{}·E{}", dt + 1, ds + 1);
        } else {
            self.msg = format!("Clip copiado a P{}·E{}", dt + 1, ds + 1);
        }
        shared.apply_control(ControlCmd::SelectClip { track: dt as u8, scene: ds as u8 });
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context, shared: &SessionShared) {
        use lanzadera_core::control::TRACK_CURSOR;
        egui::menu::bar(ui, |ui| {
            ui.menu_button("Archivo", |ui| {
                if ui.button("Nuevo                  Ctrl+N").clicked() { self.request_action(shared, PendingAction::New); ui.close_menu(); }
                if ui.button("Abrir…                 Ctrl+O").clicked() { self.request_action(shared, PendingAction::Open); ui.close_menu(); }
                ui.menu_button("Abrir reciente", |ui| {
                    if self.recent_projects.is_empty() {
                        ui.add_enabled(false, egui::Button::new("(sin proyectos recientes)"));
                    }
                    let recents = self.recent_projects.clone();
                    for p in recents {
                        if ui.button(trunc(&p, 48)).on_hover_text(p.clone()).clicked() {
                            self.request_action(shared, PendingAction::OpenPath(p));
                            ui.close_menu();
                        }
                    }
                });
                ui.separator();
                if ui.button("Guardar                Ctrl+S").clicked() { self.project_save_or_ask(shared); ui.close_menu(); }
                if ui.button("Guardar como…     Ctrl+Mayús+S").clicked() { self.spawn_file_dialog(DialogKind::Save); ui.close_menu(); }
                ui.separator();
                if ui.button("Abrir por ruta…").on_hover_text("Alternativa si no aparecen los diálogos de archivo del sistema").clicked() {
                    self.path_prompt = Some((DialogKind::Open, self.project_path.clone()));
                    ui.close_menu();
                }
                if ui.button("Guardar como por ruta…").clicked() {
                    self.path_prompt = Some((DialogKind::Save, self.project_path.clone()));
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Exportar Groove a MIDI (.mid)").clicked() {
                    self.msg = match shared.groove.export_midi(false, true) {
                        Ok(p) => format!("Exportado {}", p.display()),
                        Err(e) => format!("⚠ No se pudo exportar: {}", e),
                    };
                    ui.close_menu();
                }
                ui.add_enabled(false, egui::Button::new("Importar MIDI / audio…"))
                    .on_disabled_hover_text("Llega en la Fase 9");
                ui.add_enabled(false, egui::Button::new("Exportar audio (bounce)…"))
                    .on_disabled_hover_text("Llega en la Fase 9");
                ui.separator();
                if ui.button("Salir").clicked() { self.request_action(shared, PendingAction::Quit); ui.close_menu(); }
            });

            ui.menu_button("Edición", |ui| {
                if ui.button("Copiar clip       Ctrl+C").clicked() { self.copy_selected_clip(shared); ui.close_menu(); }
                if ui.button("Cortar clip       Ctrl+X").clicked() { self.cut_selected_clip(shared); ui.close_menu(); }
                if ui.button("Pegar clip        Ctrl+V").clicked() { self.paste_clip(shared); ui.close_menu(); }
                if ui.button("Duplicar           Ctrl+D").clicked() { self.duplicate_selected_clip(shared); ui.close_menu(); }
                if ui.button("Eliminar            Supr").clicked() { self.delete_selected_clip(shared); ui.close_menu(); }
                ui.separator();
                ui.add_enabled(false, egui::Button::new("Deshacer          Ctrl+Z"))
                    .on_disabled_hover_text("Pendiente: historial de undo/redo");
                ui.add_enabled(false, egui::Button::new("Rehacer            Ctrl+Y"))
                    .on_disabled_hover_text("Pendiente: historial de undo/redo");
            });

            ui.menu_button("Ver", |ui| {
                ui.checkbox(&mut self.windows.clip_launch_visible, "Clip Launch");
                let mut g = self.windows.groove_machine_visible;
                if ui.checkbox(&mut g, "Groove Machine").changed() {
                    self.windows.groove_machine_visible = g;
                    if g { shared.caja_sched.enabled.store(true, std::sync::atomic::Ordering::Relaxed); }
                    shared.groove_machine_visible.store(g, std::sync::atomic::Ordering::Relaxed);
                }
                ui.checkbox(&mut self.windows.inspector_visible, "Inspector");
                ui.checkbox(&mut self.windows.browser_visible, "Browser");
                ui.checkbox(&mut self.windows.mixer_visible, "Mixer");
                ui.separator();
                if ui.button("Guardar disposición de ventanas como plantilla").clicked() {
                    self.sync_window_layout_to_state(shared);
                    let wl = shared.state.lock().unwrap_or_else(|e| e.into_inner()).window_layout.clone();
                    lanzadera_core::config::update(|c| c.window_layout_template = Some(wl));
                    self.msg = "Plantilla de ventanas guardada".into();
                    ui.close_menu();
                }
                if ui.button("Aplicar plantilla de ventanas").clicked() {
                    if let Some(tpl) = lanzadera_core::config::get().window_layout_template {
                        shared.edit(|st| st.window_layout = tpl);
                        self.window_layout_apply_frames = 3;
                        self.apply_window_layout_from_state(shared);
                        self.msg = "Plantilla de ventanas aplicada".into();
                    } else {
                        self.msg = "No hay plantilla de ventanas guardada".into();
                    }
                    ui.close_menu();
                }
            });

            ui.menu_button("Pista", |ui| {
                if ui.button("Armar / desarmar pista seleccionada").clicked() {
                    shared.apply_control(ControlCmd::ToggleArm { track: TRACK_CURSOR });
                    ui.close_menu();
                }
                if ui.button("Silenciar (Mute)").clicked() {
                    shared.apply_control(ControlCmd::ToggleMute { track: TRACK_CURSOR });
                    ui.close_menu();
                }
                if ui.button("Solo").clicked() {
                    shared.apply_control(ControlCmd::ToggleSolo { track: TRACK_CURSOR });
                    ui.close_menu();
                }
                if ui.button("Detener pista seleccionada").clicked() {
                    shared.apply_control(ControlCmd::StopClip { track: TRACK_CURSOR });
                    ui.close_menu();
                }
                ui.separator();
                ui.add_enabled(false, egui::Button::new("Nueva pista MIDI / de audio"))
                    .on_disabled_hover_text("Llega en la Fase 8");
            });

            ui.menu_button("Opciones", |ui| {
                if ui.button("Preferencias…").clicked() { self.prefs_open = true; ui.close_menu(); }
                if ui.button("Controladores MIDI — buscar…").clicked() {
                    self.prefs_open = true;
                    self.prefs_tab = PrefsTab::Midi;
                    lanzadera_engine::midi_ctl::request(lanzadera_engine::midi_ctl::MidiRequest::Rescan);
                    ui.close_menu();
                }
                if ui.button("Carpetas de plugins…").clicked() {
                    self.prefs_open = true;
                    self.prefs_tab = PrefsTab::Plugins;
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Audio y latencia…").clicked() {
                    self.prefs_open = true;
                    self.prefs_tab = PrefsTab::Audio;
                    ui.close_menu();
                }
                ui.add_enabled(false, egui::Button::new("Entradas de audio por canal…"))
                    .on_disabled_hover_text("Llega en la Fase 4");
                ui.add_enabled(false, egui::Button::new("Atajos de teclado (editar)…"))
                    .on_disabled_hover_text("Llega más adelante");
            });

            ui.menu_button("Ayuda", |ui| {
                if ui.button("Atajos de teclado").clicked() { self.show_shortcuts = true; ui.close_menu(); }
                if ui.button("Acerca de Lanzadera").clicked() { self.show_about = true; ui.close_menu(); }
            });
        });
    }

    fn help_windows(&mut self, ctx: &egui::Context) {
        if self.show_shortcuts {
            let mut open = true;
            egui::Window::new("Atajos de teclado").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
                egui::Grid::new("atajos_grid").num_columns(2).spacing([24.0, 4.0]).show(ui, |ui| {
                    for (k, d) in [
                        ("Ctrl+N", "Proyecto nuevo"),
                        ("Ctrl+O", "Abrir proyecto"),
                        ("Ctrl+S", "Guardar"),
                        ("Ctrl+Mayús+S", "Guardar como…"),
                        ("Ctrl+C / Ctrl+X / Ctrl+V", "Copiar / cortar / pegar clip"),
                        ("Ctrl+D", "Duplicar clip seleccionado"),
                        ("Supr / Retroceso", "Eliminar clip seleccionado"),
                        ("Alt + arrastrar clip", "Copiar clip a otra ranura"),
                        ("Arrastrar clip", "Mover clip a otra ranura"),
                        ("D / S / C", "Piano roll: dibujar / seleccionar / cortar"),
                        ("Ctrl+A", "Piano roll: seleccionar todo"),
                        ("Supr", "Piano roll: borrar la selección"),
                    ] {
                        ui.label(egui::RichText::new(k).strong());
                        ui.label(d);
                        ui.end_row();
                    }
                });
            });
            self.show_shortcuts = open;
        }
        if self.show_about {
            let mut open = true;
            egui::Window::new("Acerca de Lanzadera").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
                ui.label(egui::RichText::new("LANZADERA").size(16.0).strong().color(T.accent_gold));
                ui.label(format!("Versión {}", env!("CARGO_PKG_VERSION")));
                ui.label("Estación de trabajo para improvisación musical (clip launch).");
            });
            self.show_about = open;
        }
    }

    // ── TOP BAR (estilo DAW: transporte | vistas | quantize | proyecto) ────
    fn top_bar(&mut self, ctx: &egui::Context, shared: &SessionShared, audio_on: bool) {
        let mut cmds: Vec<ControlCmd> = Vec::new();
        let mut new_q: Option<Quantize> = None;
        egui::TopBottomPanel::top("top_bar")
            .frame(egui::Frame::none()
                .fill(T.transport_bg)
                .inner_margin(egui::Margin::symmetric(8.0, 4.0))
                .stroke(egui::Stroke::new(1.0, T.strip_border)))
            .show(ctx, |ui| {
            // Fila 0: barra de menús (Archivo · Edición · Ver · Pista · Opciones · Ayuda)
            self.menu_bar(ui, ctx, shared);
            ui.add_space(2.0);
            // Fila 1: identidad + transporte + metronomo + vistas
            ui.horizontal(|ui| {
                // Identidad
                ui.label(egui::RichText::new("LANZADERA")
                    .size(14.0).strong().color(T.accent_gold));
                ui.add_space(6.0);
                ui.separator();

                // ── Transporte compacto (estilo Bitwig / Ableton) ──
                let (mut tempo, playing, recording, q, rec_armed, metronome) = {
                    let st = shared.state.lock().unwrap();
                    let (rt, _, _) = st.record_target();
                    let armed = st.tracks.get(rt).map(|tr| tr.arm).unwrap_or(false);
                    (st.tempo, st.playing, st.recording, st.quantize, armed, st.metronome)
                };

                let transport_btn = |ui: &mut egui::Ui, label: &str, active: bool, col: Color32| -> bool {
                    let fill = if active { col.gamma_multiply(0.55) } else { T.panel_raised };
                    ui.add(egui::Button::new(egui::RichText::new(label).size(13.0)).fill(fill).min_size(vec2(36.0, 26.0))).clicked()
                };

                if transport_btn(ui, if playing { "⏸" } else { "▶" }, playing, T.clip_playing) {
                    cmds.push(if playing { ControlCmd::Stop } else { ControlCmd::Play });
                }
                if transport_btn(ui, "⏹", false, T.text_dim) {
                    lanzadera_engine::audio_io::STOP_ALL_REQ.store(true, std::sync::atomic::Ordering::Relaxed);
                    cmds.push(ControlCmd::StopAll);
                }
                if ui.add_enabled(rec_armed || recording,
                    egui::Button::new(egui::RichText::new("⏺").size(13.0))
                        .fill(if recording { T.clip_rec.gamma_multiply(0.65) } else { T.panel_raised })
                        .min_size(vec2(36.0, 26.0))
                ).clicked() {
                    cmds.push(ControlCmd::Record);
                }

                // Metrónomo (click izq: on/off, click der: menú de modo/subdivisión)
                let metro_resp = ui.add(egui::Button::new(egui::RichText::new("⏱").size(13.0))
                    .fill(if metronome { T.accent_gold.gamma_multiply(0.55) } else { T.panel_raised })
                    .min_size(vec2(36.0, 26.0)));
                if metro_resp.clicked() {
                    cmds.push(ControlCmd::ToggleMetronome);
                }
                metro_resp.context_menu(|ui| {
                    let (mut mode, mut subdiv) = {
                        let st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                        (st.metronome_mode, st.metronome_subdiv)
                    };
                    ui.label(egui::RichText::new("Metrónomo").strong());
                    ui.separator();
                    ui.label("Modo");
                    ui.radio_value(&mut mode, 0, "Siempre");
                    ui.radio_value(&mut mode, 1, "Solo count-in de grabación");
                    ui.radio_value(&mut mode, 2, "Apagado");
                    ui.separator();
                    ui.label("Subdivisión");
                    ui.radio_value(&mut subdiv, 0, "1/4");
                    ui.radio_value(&mut subdiv, 1, "1/8");
                    ui.radio_value(&mut subdiv, 2, "1/16");
                    ui.radio_value(&mut subdiv, 3, "1/32");
                    let mut changed = false;
                    let (cur_mode, cur_sub) = { let st = shared.state.lock().unwrap_or_else(|e| e.into_inner()); (st.metronome_mode, st.metronome_subdiv) };
                    if mode != cur_mode || subdiv != cur_sub { changed = true; }
                    if changed {
                        shared.edit(|st| {
                            st.metronome_mode = mode;
                            st.metronome_subdiv = subdiv;
                            st.metronome = mode != 2;
                        });
                    }
                });
                if metronome {
                    let (mode, subdiv) = { let st = shared.state.lock().unwrap_or_else(|e| e.into_inner()); (st.metronome_mode, st.metronome_subdiv) };
                    let sfx = match subdiv { 2 => "1/16", 3 => "1/32", 1 => "1/8", _ => "1/4" };
                    let mfx = match mode { 1 => "REC", 2 => "OFF", _ => "ON" };
                    ui.label(egui::RichText::new(format!("MET {} {}", mfx, sfx)).size(9.0).color(T.accent_gold));
                }

                ui.separator();

                // Tempo
                ui.label(egui::RichText::new("BPM").size(10.0).color(T.text_dim));
                if ui.add(egui::DragValue::new(&mut tempo).speed(0.15).range(20.0..=666.0)
                    .custom_formatter(|v, _| format!("{:.1}", v))
                ).changed() {
                    cmds.push(ControlCmd::SetTempo { bpm: tempo as f32 });
                }

                ui.separator();

                // Quantize global
                ui.label(egui::RichText::new("Q").size(10.0).color(T.text_dim));
                let mut q_mut = q;
                egui::ComboBox::from_id_source("quantize_global")
                    .width(90.0)
                    .selected_text(q_label(q))
                    .show_ui(ui, |ui| {
                        for (qq, lbl) in Q_LIST {
                            ui.selectable_value(&mut q_mut, qq, lbl);
                        }
                    });
                if q_mut != q { new_q = Some(q_mut); }

                ui.separator();

                // ── Vistas (toggle de paneles) ──
                let vis_btn = |ui: &mut egui::Ui, label: &str, visible: bool| -> bool {
                    let fill = if visible { T.accent.gamma_multiply(0.35) } else { T.panel_raised };
                    ui.add(egui::Button::new(egui::RichText::new(label).size(11.0)).fill(fill)).clicked()
                };

                if vis_btn(ui, "Clip Launch", self.windows.clip_launch_visible) {
                    self.windows.clip_launch_visible = !self.windows.clip_launch_visible;
                }
                let groove_on = shared.caja_sched.enabled.load(std::sync::atomic::Ordering::Relaxed);
                let groove_step = groove_head(shared);
                let groove_label = if groove_on && groove_step != 255 {
                    format!("GM {}/16", groove_step + 1)
                } else if groove_on {
                    "GM ON".to_string()
                } else {
                    "Groove".to_string()
                };
                if vis_btn(ui, &groove_label, self.windows.groove_machine_visible) {
                    let v = !self.windows.groove_machine_visible;
                    self.windows.groove_machine_visible = v;
                    if v {
                        shared.caja_sched.enabled.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    shared.groove_machine_visible.store(v, std::sync::atomic::Ordering::Relaxed);
                }
                if vis_btn(ui, "🥁", groove_on) {
                    shared.caja_sched.enabled.store(!groove_on, std::sync::atomic::Ordering::Relaxed);
                }
                if vis_btn(ui, "Piano Roll", self.roll.open) {
                    if self.roll.open {
                        self.roll.open = false;
                    } else {
                        // Abre el clip que ya tenía el piano roll, o el clip seleccionado.
                        let (t, s) = match self.roll.target {
                            Some(ts) => ts,
                            None => { let (t, s, _) = shared.state.lock().unwrap().record_target(); (t, s) }
                        };
                        self.roll.open_for(t, s);
                    }
                }
                if vis_btn(ui, "Inspector", self.windows.inspector_visible) {
                    self.windows.inspector_visible = !self.windows.inspector_visible;
                }
                if vis_btn(ui, "Browser", self.windows.browser_visible) {
                    self.windows.browser_visible = !self.windows.browser_visible;
                }
                if vis_btn(ui, "Mixer", self.windows.mixer_visible) {
                    self.windows.mixer_visible = !self.windows.mixer_visible;
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(
                        if audio_on { "● AUDIO" } else { "○ SIN AUDIO" }
                    ).size(10.0).color(if audio_on { T.accent_green } else { T.text_dim }));
                    let status = match &*shared.status.lock().unwrap() {
                        LoadStatus::Idle => String::new(),
                        LoadStatus::Loading { .. } => "Cargando…".into(),
                        LoadStatus::Ready => String::new(),
                        LoadStatus::Error(e) => format!("⚠ {}", e),
                    };
                    if !status.is_empty() {
                        ui.label(egui::RichText::new(status).color(T.clip_rec).size(10.0));
                    }
                });
            });

            // Fila 2: nombre del proyecto + último mensaje
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Proyecto").size(10.0).color(T.text_dim));
                ui.label(egui::RichText::new(format!("{}{}", self.project_name(), if self.dirty { " *" } else { "" })).size(11.0).strong().color(T.text_bright))
                    .on_hover_text(self.project_path.clone());
                if !self.msg.is_empty() {
                    ui.label(egui::RichText::new(&self.msg).color(T.accent).size(11.0));
                }
            });
        });
        for c in cmds { shared.apply_control(c); }
        if let Some(q) = new_q { shared.edit(|st| st.quantize = q); }
    }

    // ── CLIP GRID ──────────────────────────────────────────────────────────
        fn clip_grid(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context, shared: &SessionShared, _audio_on: bool) {
        let (mut snapshot, n_scenes) = {
            let st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            (crate::patches::build_snapshot::build_snapshot(&st), st.scenes.len())
        };
        snapshot.scene_count = n_scenes.max(1);
        // Mapear índice visible -> índice real (Groove se renderiza como fila especial).
        let (groove_idx, vis_to_real): (Option<usize>, Vec<usize>) = {
            let st = shared.state.lock().unwrap();
            let gi = st.tracks.iter().position(|t| t.name.eq_ignore_ascii_case("Groove"));
            let map = st.tracks.iter().enumerate()
                .filter(|(i, _)| Some(*i) != gi)
                .map(|(i, _)| i)
                .collect();
            (gi, map)
        };

        // El estado REAL lo publica el ClipEngine via rt, no SessionState.
        if _audio_on {
            use crate::patches::session_snapshot::ClipStateView;
            for (vi, tr) in snapshot.tracks.iter_mut().enumerate() {
                let Some(&ti) = vis_to_real.get(vi) else { continue; };
                let active = shared.rt.active_scene(ti);
                for (si, slot) in tr.clips.iter_mut().enumerate() {
                    let Some(c) = slot.as_mut() else { continue };
                    c.state = match shared.rt.clip_state(ti, si) {
                        ClipState::Empty => ClipStateView::Empty,
                        ClipState::Triggered => ClipStateView::Triggered,
                        ClipState::Playing => ClipStateView::Playing,
                        ClipState::Stopping => ClipStateView::Stopping,
                        ClipState::Recording => ClipStateView::Recording,
                        ClipState::Overdubbing => ClipStateView::Overdubbing,
                        ClipState::RecQueued => ClipStateView::RecQueued,
                    };
                    let lt = (c.loop_bars.max(1) as u32 * TICKS_PER_BAR as u32).max(1);
                    c.progress = if active == Some(si as u8) {
                        (shared.rt.playhead(ti) as f32 / lt as f32).clamp(0.0, 1.0)
                    } else { 0.0 };
                }
            }
            if let (Some(g), Some(ti)) = (&mut snapshot.groove, groove_idx) {
                let active = shared.rt.active_scene(ti);
                for (si, slot) in g.clips.iter_mut().enumerate() {
                    let Some(c) = slot.as_mut() else { continue };
                    c.state = match shared.rt.clip_state(ti, si) {
                        ClipState::Empty => ClipStateView::Empty,
                        ClipState::Triggered => ClipStateView::Triggered,
                        ClipState::Playing => ClipStateView::Playing,
                        ClipState::Stopping => ClipStateView::Stopping,
                        ClipState::Recording => ClipStateView::Recording,
                        ClipState::Overdubbing => ClipStateView::Overdubbing,
                        ClipState::RecQueued => ClipStateView::RecQueued,
                    };
                    let lt = (c.loop_bars.max(1) as u32 * TICKS_PER_BAR as u32).max(1);
                    c.progress = if active == Some(si as u8) {
                        (shared.rt.playhead(ti) as f32 / lt as f32).clamp(0.0, 1.0)
                    } else { 0.0 };
                }
            }
        }

        // C2b: picos REALES del audio (el snapshot no ve el SampleStore).
        for (vi, tr) in snapshot.tracks.iter_mut().enumerate() {
            let Some(&ti) = vis_to_real.get(vi) else { continue; };
            for (si, slot) in tr.clips.iter_mut().enumerate() {
                let Some(c) = slot.as_mut() else { continue };
                if let Some(sd) = shared.samples.load(ti, si) {
                    let mx = sd.peaks.iter().fold(0.0f32, |a, &b| a.max(b));
                    if mx > 0.0005 || c.peaks.is_empty() { c.peaks = sd.peaks.clone(); }
                }
            }
        }
        if let (Some(g), Some(ti)) = (&mut snapshot.groove, groove_idx) {
            for (si, slot) in g.clips.iter_mut().enumerate() {
                let Some(c) = slot.as_mut() else { continue; };
                if let Some(sd) = shared.samples.load(ti, si) {
                    let mx = sd.peaks.iter().fold(0.0f32, |a, &b| a.max(b));
                    if mx > 0.0005 || c.peaks.is_empty() { c.peaks = sd.peaks.clone(); }
                }
            }
        }
        let mut actions = crate::patches::clip_grid::ClipGridActions::default();
        crate::patches::clip_grid::render_session_matrix(ui, &snapshot, &mut actions);

        use lanzadera_core::control::ControlCmd;
        use lanzadera_core::clip::Quantize;
        let map_t = |vt: usize| -> usize { vis_to_real.get(vt).copied().unwrap_or(vt) };

        if let Some(t) = actions.select_track {
            let t = map_t(t);
            shared.edit(|st| { st.selected_track = t; });
            self.inspector_focus = InspectorFocus::Track;
        }
        if let Some((t, s)) = actions.select_clip {
            let t = map_t(t);
            shared.apply_control(ControlCmd::SelectClip { track: t as u8, scene: s as u8 });
            self.inspector_focus = InspectorFocus::Clip;
        }
        if let Some((t, s)) = actions.create_clip {
            let t = map_t(t);
            shared.apply_control(ControlCmd::CreateClip { track: t as u8, scene: s as u8, bars: 8 });
            self.inspector_focus = InspectorFocus::Clip;
        }
        if let Some((t, s)) = actions.create_and_record_clip {
            let t = map_t(t);
            shared.apply_control(ControlCmd::CreateClip { track: t as u8, scene: s as u8, bars: 8 });
            // CreateClip en pista armada ya arma RecQueued; Record lo inicia según estado actual.
            shared.apply_control(ControlCmd::Record);
            self.inspector_focus = InspectorFocus::Clip;
        }
        if let Some((t, s)) = actions.launch_clip {
            let t = map_t(t);
            // El cuerpo del clip ya no lanza; mantenemos la acción por compatibilidad.
            shared.apply_control(ControlCmd::SelectClip { track: t as u8, scene: s as u8 });
            self.inspector_focus = InspectorFocus::Clip;
        }
        if let Some((t, s)) = actions.toggle_clip {
            let t = map_t(t);
            // Botón de estado: play/stop del clip, o arm/rec según el flujo del core.
            shared.apply_control(ControlCmd::LaunchClip { track: t as u8, scene: s as u8 });
            shared.apply_control(ControlCmd::SelectClip { track: t as u8, scene: s as u8 });
            self.inspector_focus = InspectorFocus::Clip;
        }
        if let Some(si) = actions.launch_scene {
            shared.apply_control(ControlCmd::LaunchScene { scene: si as u8 });
        }
        if let Some(t) = actions.toggle_arm {
            let t = map_t(t);
            shared.apply_control(ControlCmd::ToggleArm { track: t as u8 });
        }
        if let Some(t) = actions.toggle_mute {
            let t = map_t(t);
            shared.apply_control(ControlCmd::ToggleMute { track: t as u8 });
        }
        if let Some(t) = actions.toggle_solo {
            let t = map_t(t);
            shared.apply_control(ControlCmd::ToggleSolo { track: t as u8 });
        }
        if let Some(t) = actions.stop_track {
            let t = map_t(t);
            shared.apply_control(ControlCmd::StopClip { track: t as u8 });
        }
        if actions.stop_all {
            shared.apply_control(ControlCmd::StopAll);
        }
        if let Some(q) = actions.set_quantize {
            let q_enum = match q {
                1 => Quantize::One,
                2 => Quantize::Two,
                4 => Quantize::Four,
                _ => Quantize::One,
            };
            shared.edit(|st| st.quantize = q_enum);
        }
        if let Some((t, s)) = actions.delete_clip {
            let t = map_t(t);
            shared.apply_control(ControlCmd::StopClip { track: t as u8 });
            shared.edit(|st| {
                if let Some(tr) = st.tracks.get_mut(t) {
                    if let Some(slot) = tr.clips.get_mut(s) { *slot = None; }
                }
                if st.selected_clip == Some((t, s)) { st.selected_clip = None; }
            });
            shared.sync_midi(t, s);
            // sync_midi solo limpia las notas: sin esto el audio viejo queda en el SampleStore
            shared.samples.clear(t, s);
        }
        if let Some((t, s)) = actions.duplicate_clip {
            let t = map_t(t);
            let target = shared.edit(|st| {
                if let Some(tr) = st.tracks.get_mut(t) {
                    let copy = tr.clips.get(s).and_then(|c| c.clone());
                    if let Some(c) = copy {
                        if let Some(empty) = tr.clips.iter_mut().position(|x| x.is_none()) {
                            tr.clips[empty] = Some(c);
                            return Some(empty);
                        }
                    }
                }
                None
            });
            if let Some(s2) = target {
                if let Some(m) = shared.notes.load(t, s) { shared.notes.store(t, s2, m); }
                if let Some(sd) = shared.samples.load(t, s) { shared.samples.store(t, s2, sd); }
                shared.sync_midi(t, s2);
            }
        }
        if let Some((st_t, st_s, dt, ds, copy)) = actions.move_clip {
            self.move_or_copy_clip(shared, map_t(st_t), st_s, map_t(dt), ds, copy);
        }
        if let Some((t, s)) = actions.copy_clip {
            let t = map_t(t);
            shared.apply_control(ControlCmd::SelectClip { track: t as u8, scene: s as u8 });
            self.copy_selected_clip(shared);
        }
        if let Some((t, s)) = actions.cut_clip {
            let t = map_t(t);
            shared.apply_control(ControlCmd::SelectClip { track: t as u8, scene: s as u8 });
            self.cut_selected_clip(shared);
        }
        if let Some((t, s)) = actions.paste_clip {
            let t = map_t(t);
            shared.apply_control(ControlCmd::SelectClip { track: t as u8, scene: s as u8 });
            self.paste_clip(shared);
        }
        if let Some((t, s)) = actions.open_piano_roll {
            let t = map_t(t);
            self.roll.open_for(t, s);
            shared.apply_control(ControlCmd::SelectClip { track: t as u8, scene: s as u8 });
            self.inspector_focus = InspectorFocus::Clip;
        }
        if let Some((t, s)) = actions.open_audio_modal {
            let t = map_t(t);
            self.audio_modal = Some((t, s));
            self.audio_path_input.clear();
        }
        if let Some((t, s, path)) = actions.load_file {
            let t = map_t(t);
            shared.request_load(t, s, path);
        }
        if let Some(scene) = actions.export_groove_to_scene {
            self.export_groove_to_clip_launch(shared, scene);
        }
        if actions.add_scene {
            let ok = shared.edit(|st| {
                let n = st.scenes.len();
                st.push_scene(lanzadera_core::Scene {
                    name: format!("Escena {}", n + 1), ..Default::default()
                })
            });
            self.msg = if ok { "Escena añadida".into() }
                       else { format!("Máximo {} escenas", lanzadera_core::MAX_SCENES) };
        }
        if let Some(idx) = actions.remove_scene {
            let n_tracks = { shared.state.lock().unwrap().tracks.len() };
            let ok = shared.edit(|st| st.remove_scene(idx));
            if ok {
                // Reindexar los almacenes de audio (notas/muestras) por slot.
                for t in 0..n_tracks {
                    for s in idx..lanzadera_core::MAX_SCENES - 1 {
                        match shared.notes.load(t, s + 1) {
                            Some(a) => shared.notes.store(t, s, a),
                            None => shared.notes.clear(t, s),
                        }
                        match shared.samples.load(t, s + 1) {
                            Some(a) => shared.samples.store(t, s, a),
                            None => shared.samples.clear(t, s),
                        }
                    }
                    shared.notes.clear(t, lanzadera_core::MAX_SCENES - 1);
                    shared.samples.clear(t, lanzadera_core::MAX_SCENES - 1);
                }
                shared.audio_restart.store(true, std::sync::atomic::Ordering::Relaxed);
                self.msg = format!("Escena {} eliminada", idx + 1);
            } else {
                self.msg = "No se puede eliminar: queda una sola escena".into();
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(dead_code)] // celda antigua: el render actual vive en patches/clip_grid.rs
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

        // Arrastre interno desde el browser (Fase G3). Se resuelve antes que
        // el drop de archivos del SO para que ambos caminos convivan.
        // Mismo destino: la cola `loads`, que el loader reparte segun la
        // extension (.mid -> notas, audio -> SampleStore).
        if egui::DragAndDrop::has_payload_of_type::<BrowserFile>(ctx) {
            // rect_contains_pointer respeta clip_rect y layer: si el grid esta
            // scrolleado o tapado por otra ventana, no da falso positivo.
            if ui.rect_contains_pointer(rect) {
                p.rect_stroke(rect, Rounding::same(4.0), Stroke::new(2.0, T.accent));
                if ctx.input(|i| i.pointer.any_released()) {
                    if let Some(dropped) = egui::DragAndDrop::take_payload::<BrowserFile>(ctx) {
                        loads.push((t, s, dropped.0.clone()));
                    }
                }
            }
        }

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

    // ── MIXER (strips estilo consola analógica) ────────────────────────────
    fn mixer(&mut self, ui: &mut egui::Ui, shared: &SessionShared, audio_on: bool) {
        let n = { shared.state.lock().unwrap().tracks.len() };
        let mut cmds = Vec::new();
        let mut add_track = false;
        let mut remove_track: Option<usize> = None;

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("MIXER").size(11.0).strong().color(T.accent_gold));
            ui.separator();
            if ui.button("+ Pista").on_hover_text("Añadir pista de audio/instrumento").clicked() {
                add_track = true;
            }
            if n > 1 {
                if ui.button("− Pista").on_hover_text("Eliminar la pista seleccionada").clicked() {
                    let sel = shared.state.lock().unwrap().selected_track;
                    remove_track = Some(sel.min(n.saturating_sub(1)));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(format!("{} pistas", n)).size(10.0).color(T.text_dim));
            });
        });
        ui.add_space(4.0);

        egui::ScrollArea::horizontal().id_source("mixer_scroll").show(ui, |ui| {
            ui.horizontal_top(|ui| {
                for t in 0..n { self.strip(ui, shared, Some(t), audio_on, &mut cmds); }
                self.strip(ui, shared, None, audio_on, &mut cmds);
                self.groove_master_strip(ui, shared, &mut cmds);
            });
        });
        for c in cmds { shared.apply_control(c); }
        if add_track {
            shared.edit(|st| {
                let mut tr = lanzadera_core::Track::default();
                tr.name = format!("Pista {}", st.tracks.len() + 1);
                tr.kind = lanzadera_core::TrackKind::Audio;
                let _ = st.push_track(tr);
            });
        }
        if let Some(idx) = remove_track {
            shared.edit(|st| {
                if st.tracks.len() > 1 && idx < st.tracks.len() {
                    st.tracks.remove(idx);
                    if st.selected_track >= st.tracks.len() {
                        st.selected_track = st.tracks.len().saturating_sub(1);
                    }
                }
            });
        }
    }

    /// Master del Groove Machine: regula SOLO la batería. El MASTER general afecta a todo.
    fn groove_master_strip(&mut self, ui: &mut egui::Ui, shared: &SessionShared, cmds: &mut Vec<ControlCmd>) {
        let mut vol = shared.state.lock().unwrap().groove_master;
        let level = shared.rt.groove_peak();
        egui::Frame::none()
            .fill(T.strip_bg)
            .rounding(Rounding::same(5.0))
            .stroke(egui::Stroke::new(1.0, T.accent_gold))
            .inner_margin(5.0)
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.set_width(68.0);
                    ui.label(egui::RichText::new("GROOVE").size(11.0).color(T.accent_gold).strong())
                        .on_hover_text("Master del Groove Machine (slider del MK3)");
                    ui.add_space(50.0);
                    if widgets::fader(ui, &mut vol, 120.0, level) {
                        cmds.push(ControlCmd::SetGrooveMasterVol { value: vol });
                    }
                    let db = if vol <= 0.0001 { "-inf".to_string() } else { format!("{:.1}", 20.0 * vol.log10()) };
                    ui.label(egui::RichText::new(db).size(9.0).color(T.text_dim));
                });
            });
    }

    fn strip(&mut self, ui: &mut egui::Ui, shared: &SessionShared, track: Option<usize>,
             audio_on: bool, cmds: &mut Vec<ControlCmd>) {
        let (name, mut vol, mut pan, mute, solo, arm, selected) = match track {
            Some(t) => {
                let st = shared.state.lock().unwrap();
                let tr = &st.tracks[t];
                (tr.name.clone(), tr.volume, tr.pan, tr.mute, tr.solo, tr.arm, st.selected_track == t)
            }
            None => {
                let st = shared.state.lock().unwrap();
                ("MASTER".to_string(), st.master_volume, 0.0, false, false, false, false)
            }
        };
        let level = if audio_on { shared.rt.peak(track.unwrap_or(MAX_TRACKS)) } else { 0.0 };
        let border = if selected {
            egui::Stroke::new(1.5, T.accent_gold)
        } else {
            egui::Stroke::new(1.0, T.strip_border)
        };
        egui::Frame::none()
            .fill(T.strip_bg)
            .rounding(Rounding::same(5.0))
            .stroke(border)
            .inner_margin(5.0)
            .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_width(68.0);

                let head_col = if selected { T.accent_gold } else { T.text_bright };
                let head = ui.add(
                    egui::Label::new(egui::RichText::new(trunc(&name, 9)).size(11.0).color(head_col).strong())
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
                } else {
                    ui.add_space(50.0);
                }

                if widgets::fader(ui, &mut vol, 120.0, level) {
                    cmds.push(match track {
                        Some(t) => ControlCmd::SetVolume { track: t as u8, value: vol },
                        None => ControlCmd::SetMasterVol { value: vol },
                    });
                }
                let db = if vol <= 0.0001 { "-inf".to_string() } else { format!("{:.1}", 20.0 * vol.log10()) };
                ui.label(egui::RichText::new(db).size(9.0).color(T.text_dim));

                if let Some(t) = track {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        if ui.add(egui::Button::new(egui::RichText::new("M").size(11.0))
                            .fill(if mute { Color32::from_rgb(240, 200,  70).gamma_multiply(0.85) } else { T.panel_raised })
                            .min_size(vec2(20.0, 18.0))).clicked()
                        {
                            cmds.push(ControlCmd::ToggleMute { track: t as u8 });
                        }
                        if ui.add(egui::Button::new(egui::RichText::new("S").size(11.0))
                            .fill(if solo { Color32::from_rgb(240, 152,  72).gamma_multiply(0.85) } else { T.panel_raised })
                            .min_size(vec2(20.0, 18.0))).clicked()
                        {
                            cmds.push(ControlCmd::ToggleSolo { track: t as u8 });
                        }
                        if ui.add(egui::Button::new(egui::RichText::new("R").size(11.0))
                            .fill(if arm { T.clip_rec.gamma_multiply(0.7) } else { T.panel_raised })
                            .min_size(vec2(20.0, 18.0)))
                            .on_hover_text("Armar para grabar")
                            .clicked()
                        {
                            cmds.push(ControlCmd::ToggleArm { track: t as u8 });
                        }
                    });

                    // Mono: suma L+R (para una entrada mono que llegó a un solo canal).
                    let mono = shared.state.lock().unwrap().tracks.get(t).map(|tr| tr.mono).unwrap_or(false);
                    if ui.add(egui::Button::new(egui::RichText::new("Mono").size(10.0))
                        .fill(if mono { T.accent.gamma_multiply(0.6) } else { T.panel_raised })
                        .min_size(vec2(62.0, 16.0)))
                        .on_hover_text("Suma L+R en ambos lados: útil para una entrada mono grabada en un solo canal")
                        .clicked()
                    {
                        shared.edit(|st| { if let Some(tr) = st.tracks.get_mut(t) { tr.mono = !tr.mono; } });
                    }
                // Canal de entrada que alimenta esta pista (Entrada 1, 2, ...).
                let n_in = shared.input_channels.load(std::sync::atomic::Ordering::Relaxed) as usize;
                if n_in > 1 {
                    let cur_ch = shared.state.lock().unwrap().tracks.get(t)
                        .map(|tr| tr.input_channel).unwrap_or(0) as usize;
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("CH").size(9.0).color(T.text_dim));
                        egui::ComboBox::from_id_source(format!("in_ch_{}", t))
                            .width(46.0)
                            .selected_text(egui::RichText::new(format!("{}", cur_ch + 1)).size(9.0))
                            .show_ui(ui, |ui| {
                                for c in 0..n_in {
                                    if ui.selectable_label(cur_ch == c, format!("Entrada {}", c + 1)).clicked() {
                                        shared.edit(|st| {
                                            if let Some(tr) = st.tracks.get_mut(t) { tr.input_channel = c as u8; }
                                        });
                                    }
                                }
                            });
                    });
                }
                    // Entrada de audio (preparar pista para loops analógicos)
                    ui.add_space(2.0);
                    ui.label(egui::RichText::new("IN").size(9.0).color(T.text_dim));
                    let cur_in = {
                        shared.state.lock().unwrap().tracks.get(t)
                            .and_then(|tr| tr.input_device.clone())
                    };
                    let label = cur_in.as_deref().map(|s| trunc(s, 8)).unwrap_or_else(|| "Default".into());
                    egui::ComboBox::from_id_source(format!("in_sel_{}", t))
                        .width(58.0)
                        .selected_text(egui::RichText::new(label).size(9.0))
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(cur_in.is_none(), "Default").clicked() {
                                shared.edit(|st| {
                                    if let Some(tr) = st.tracks.get_mut(t) { tr.input_device = None; }
                                });
                            }
                            for name in lanzadera_engine::audio_input::list_input_devices() {
                                if ui.selectable_label(cur_in.as_deref() == Some(name.as_str()), &name).clicked() {
                                    shared.edit(|st| {
                                        if let Some(tr) = st.tracks.get_mut(t) {
                                            tr.input_device = Some(name.clone());
                                        }
                                    });
                                }
                            }
                        });
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
        // t, lo, mid, hi, mid_f, mid_q, hpf_on, lpf_on, hpf_hz, lpf_hz
        let mut eq_edit: Option<(usize, f32, f32, f32, f32, f32, bool, bool, f32, f32)> = None;
        let mut input_edit: Option<(usize, Option<String>)> = None;
        let mut open_modal = None;
        let mut open_roll = None;
        let plugin_load: Option<(usize, String)> = None; // el botón Cargar ya no existe
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
                // C2: vista de ondas del clip (picos del loader + playhead).
                if c.audio_path.is_some() || !c.midi_notes.is_empty() {
                    if let Some(sd) = shared.samples.load(t, s)
                        .filter(|sd| sd.peaks.iter().any(|&p| p > 0.0005) || c.midi_notes.is_empty()) {
                        let lt = (c.loop_bars.max(1) as u32 * TICKS_PER_BAR as u32).max(1);
                        let head = if shared.rt.active_scene(t) == Some(s as u8) {
                            Some((shared.rt.playhead(t) as f32 / lt as f32).clamp(0.0, 1.0))
                        } else { None };
                        let ch = sd.channels.max(1) as usize;
                        let frames = sd.data.len() / ch;
                        let secs = frames as f32 / sd.sample_rate.max(1) as f32;
                        let pmax = sd.peaks.iter().fold(0.0f32, |a, &b| a.max(b));
                        let fallback;
                        let peaks: &[f32] = if pmax > 0.0 { &sd.peaks } else {
                            fallback = crate::waveform::overview_from_samples(&sd.data, ch, 256);
                            &fallback
                        };
                        crate::waveform::view(ui, peaks, 72.0, head);
                        ui.label(egui::RichText::new(format!(
                            "{:.2} s · {} ch · {} muestras · pico {:.2}",
                            secs, ch, frames, pmax)).size(9.0).color(T.text_dim));
                    }
                }
                // Clip MIDI (o audio mudo): envolvente de las notas.
                if !c.midi_notes.is_empty() {
                    let audio_ok = shared.samples.load(t, s)
                        .map(|sd| sd.peaks.iter().any(|&p| p > 0.0005))
                        .unwrap_or(false);
                    if !audio_ok {
                        let lt = (c.loop_bars.max(1) as u32 * TICKS_PER_BAR as u32).max(1);
                        let head = if shared.rt.active_scene(t) == Some(s as u8) {
                            Some((shared.rt.playhead(t) as f32 / lt as f32).clamp(0.0, 1.0))
                        } else { None };
                        let pk = crate::waveform::overview_from_notes(&c.midi_notes, lt, 256);
                        crate::waveform::view(ui, &pk, 72.0, head);
                        ui.label(egui::RichText::new(format!("MIDI · {} notas", c.midi_notes.len()))
                            .size(9.0).color(T.text_dim));
                    }
                }
                    if let Some(p) = &c.audio_path {
                        ui.label(egui::RichText::new(format!("🎵 {}", trunc(p, 26))).size(10.0).color(T.text_dim));
                    }
                    if ui.button("📂 Cargar audio…").clicked() { open_modal = Some((t, s, c.audio_path.clone())); }
                    if c.audio_path.is_some() && ui.button("✂ Editar muestra").clicked() {
                        self.sample_editor.open_for(t, s);
                    }
                    if ui.button("🎹 Piano Roll").clicked() { open_roll = Some((t, s)); }
                    if changed { clip_edit = Some((t, s, c)); }
                } else {
                    ui.label("Slot vacío — crea un clip o arrastra un WAV.");
                }
            } else {
                ui.label("Selecciona un clip (click derecho o click en slot vacío).");
            }
            ui.separator();
            {
                let mono = shared.state.lock().unwrap().tracks.get(t).map(|tr| tr.mono).unwrap_or(false);
                let mut m = mono;
                if ui.checkbox(&mut m, "Mono (suma L+R)")
                    .on_hover_text("Para una entrada mono grabada en un solo canal: suena igual en ambos lados")
                    .changed()
                {
                    shared.edit(|st| { if let Some(tr) = st.tracks.get_mut(t) { tr.mono = m; } });
                }
            }
            ui.label(egui::RichText::new("Instrumento · Note FX · FX").size(12.0));
            if !lanzadera_engine::CLAP_ENABLED {
                ui.label(egui::RichText::new("Compila con --features clap").size(10.0).color(T.text_dim));
            }
            let cur = { shared.state.lock().unwrap().tracks.get(t).and_then(|tr| tr.plugin_path.clone()) };
            if let Some(p) = &cur {
                ui.label(egui::RichText::new(format!("🔌 {}", trunc(p, 24))).size(10.0).color(T.accent));
            }
            // Instrumento: se elige con 🔍 CLAP / 🔍 VST3. Los antiguos botones
            // Cargar / Quitar / Parámetros (ruta manual) ya no hacen falta.
            ui.horizontal_wrapped(|ui| {
                // Interfaz propia del plugin (Fase G6a). Atenuado si el plugin no
                // expone la extension GUI: mismo criterio que Rec sin arm.
                let (has_gui, gui_open) = lanzadera_engine::clap_gui::state(t);
                let label = if gui_open { "🖥 Cerrar interfaz" } else { "🖥 Interfaz" };
                if ui.add_enabled(has_gui, egui::Button::new(label)).clicked() {
                    lanzadera_engine::clap_gui::request_toggle(t);
                }
                if lanzadera_engine::CLAP_ENABLED && ui.button("🔍 CLAP").on_hover_text("Elegir instrumento CLAP").clicked() {
                    self.windows.browser_visible = true;
                    self.browser_tab = BrowserTab::Instruments;
                    self.browser_target = BrowserTarget::Instrument;
                    self.browser_mode = BrowserMode::Clap;
                    self.browser_show_all = false;
                    self.browser_list = None;
                }
                if lanzadera_engine::VST3_ENABLED && ui.button("🔍 VST3").on_hover_text("Elegir instrumento VST3").clicked() {
                    self.windows.browser_visible = true;
                    self.browser_tab = BrowserTab::Instruments;
                    self.browser_target = BrowserTarget::Instrument;
                    self.browser_mode = BrowserMode::Vst3;
                    self.browser_show_all = false;
                    self.browser_list = None;
                }
                if cur.is_some() && ui.small_button("✕").on_hover_text("Quitar instrumento (vuelve al sinte interno)").clicked() {
                    plugin_clear = Some(t);
                }
            });
            // Note FX y FX de la pista seleccionada: CADENAS de inserts (Paso 2).
        let (nfx_chain, fx_chain) = {
            let st = shared.state.lock().unwrap();
            st.tracks.get(t)
                .map(|tr| (tr.note_fx_chain.clone(), tr.fx_chain.clone()))
                .unwrap_or_default()
        };
        for (target, lbl, chain) in [
            (BrowserTarget::NoteFx, "Note FX", &nfx_chain),
            (BrowserTarget::Fx, "FX", &fx_chain),
        ] {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(lbl).size(11.0).strong());
                if ui.button(format!("＋ {}", lbl)).on_hover_text("Añadir plugin a la cadena").clicked() {
                    self.windows.browser_visible = true;
                    self.browser_tab = match target {
                        BrowserTarget::NoteFx => BrowserTab::NoteFx,
                        BrowserTarget::Fx => BrowserTab::Fx,
                        BrowserTarget::Instrument => BrowserTab::Instruments,
                    };
                    self.browser_target = target;
                    self.browser_mode = if lanzadera_engine::CLAP_ENABLED { BrowserMode::Clap } else { BrowserMode::Vst3 };
                    self.browser_show_all = false;
                    self.browser_list = None;
                }
                if !chain.is_empty() && ui.small_button("🗑").on_hover_text("Vaciar la cadena").clicked() {
                    if target == BrowserTarget::NoteFx { shared.set_note_fx(t, None, None); }
                    else { shared.set_fx(t, None, None); }
                }
            });
            for (i, slot) in chain.iter().enumerate() {
                let stem = std::path::Path::new(&slot.path)
                    .file_stem().and_then(|x| x.to_str()).unwrap_or(&slot.path).to_string();
                ui.horizontal(|ui| {
                    let gui_id = if target == BrowserTarget::NoteFx {
                        lanzadera_engine::clap_gui::nfx_gui_id(t, i)
                    } else {
                        lanzadera_engine::clap_gui::fx_gui_id(t, i)
                    };
                    let (has_gui, gui_open) = lanzadera_engine::clap_gui::state(gui_id);
                    if ui.add_enabled(has_gui, egui::Button::new(if gui_open { "🖥 Cerrar" } else { "🖥" })
                        .min_size(vec2(26.0, 18.0)))
                        .on_hover_text(if has_gui { "Abrir o cerrar la interfaz de este plugin" } else { "Sin interfaz: al pulsar verás aviso" })
                        .clicked()
                    {
                        lanzadera_engine::clap_gui::request_toggle(gui_id);
                    }
                    let mut by = slot.bypass;
                    if ui.checkbox(&mut by, "").on_hover_text("Bypass (se aplica al reiniciar el audio)").changed() {
                        let tt = target;
                        shared.edit(|st| {
                            if let Some(tr) = st.tracks.get_mut(t) {
                                let chain = if tt == BrowserTarget::NoteFx { &mut tr.note_fx_chain } else { &mut tr.fx_chain };
                                if let Some(sl) = chain.get_mut(i) { sl.bypass = by; }
                            }
                        });
                        shared.audio_restart.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    ui.label(egui::RichText::new(trunc(&stem, 14)).size(10.0));
                    if slot.state.is_some() {
                        ui.label(egui::RichText::new("●").size(9.0).color(T.accent_green))
                            .on_hover_text("Con parámetros guardados");
                    }
                    if ui.small_button("✕").on_hover_text("Quitar de la cadena").clicked() {
                        if target == BrowserTarget::NoteFx { shared.remove_note_fx(t, i); }
                        else { shared.remove_fx(t, i); }
                    }
                });
            }
        }
            ui.separator();
            ui.label(egui::RichText::new(format!("EQ · Pista {}", t + 1)).size(12.0));
            ui.label(egui::RichText::new("Rueda sobre MID = Q (campana) · drag horizontal = freq")
                .size(9.0).color(T.text_dim));
            let (mut eq_p, mut hpf_on, mut lpf_on, mut hpf_hz, mut lpf_hz) = {
                let st = shared.state.lock().unwrap();
                let tr = &st.tracks[t.min(st.tracks.len() - 1)];
                (eq_view::EqParams {
                    lo: tr.eq_low, mid: tr.eq_mid, hi: tr.eq_high,
                    mid_f: tr.eq_mid_f, mid_q: tr.eq_mid_q,
                }, tr.hpf_on, tr.lpf_on, tr.hpf_hz, tr.lpf_hz)
            };
            let mut eq_changed = false;
            // Curva ProQ-style completa (Q + freq persistentes).
            eq_changed |= eq_view::curve_full(ui, &mut eq_p);
            eq_changed |= ui.add(egui::Slider::new(&mut eq_p.lo, -15.0..=15.0).text("Low")).changed();
            eq_changed |= ui.add(egui::Slider::new(&mut eq_p.mid, -15.0..=15.0).text("Mid")).changed();
            eq_changed |= ui.add(egui::Slider::new(&mut eq_p.hi, -15.0..=15.0).text("High")).changed();
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("Mid {:.0} Hz  Q={:.2}", eq_p.mid_f, eq_p.mid_q))
                    .size(10.0).color(T.text_dim));
            });
            ui.horizontal(|ui| {
                eq_changed |= ui.checkbox(&mut hpf_on, "HPF").changed();
                eq_changed |= ui.add(egui::DragValue::new(&mut hpf_hz).range(20.0..=2000.0).suffix(" Hz")).changed();
            });
            ui.horizontal(|ui| {
                eq_changed |= ui.checkbox(&mut lpf_on, "LPF").changed();
                eq_changed |= ui.add(egui::DragValue::new(&mut lpf_hz).range(200.0..=20000.0).suffix(" Hz")).changed();
            });
            if eq_changed {
                eq_edit = Some((t, eq_p.lo, eq_p.mid, eq_p.hi, eq_p.mid_f, eq_p.mid_q,
                                hpf_on, lpf_on, hpf_hz, lpf_hz));
            }

            // Entrada de audio para grabar loops analógicos
            ui.separator();
            ui.label(egui::RichText::new("Entrada de audio").size(12.0));
            ui.label(egui::RichText::new("Dispositivo para grabar bajo / instrumentos analógicos")
                .size(9.0).color(T.text_dim));
            let devices = lanzadera_engine::audio_input::list_input_devices();
            let current_in = {
                shared.state.lock().unwrap().tracks.get(t)
                    .and_then(|tr| tr.input_device.clone())
            };
            let display = current_in.as_deref().unwrap_or("Default In");
            let mut chosen: Option<String> = current_in.clone();
            egui::ComboBox::from_id_source(format!("insp_in_{}", t))
                .selected_text(egui::RichText::new(trunc(display, 22)).size(11.0))
                .show_ui(ui, |ui| {
                    if ui.selectable_label(chosen.is_none(), "Default In").clicked() {
                        chosen = None;
                        input_edit = Some((t, None));
                    }
                    for name in &devices {
                        if ui.selectable_label(chosen.as_deref() == Some(name.as_str()), name).clicked() {
                            chosen = Some(name.clone());
                            input_edit = Some((t, Some(name.clone())));
                        }
                    }
                });
            let arm = { shared.state.lock().unwrap().tracks.get(t).map(|tr| tr.arm).unwrap_or(false) };
            if ui.add(egui::Button::new(if arm { "● Armado" } else { "○ Armar pista" })
                .fill(if arm { T.clip_rec.gamma_multiply(0.55) } else { T.panel_raised }))
                .clicked()
            {
                shared.apply_control(ControlCmd::ToggleArm { track: t as u8 });
            }

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
        if let Some((t, lo, mid, hi, mid_f, mid_q, hpf_on, lpf_on, hpf_hz, lpf_hz)) = eq_edit {
            shared.edit(|st| {
                if let Some(tr) = st.tracks.get_mut(t) {
                    tr.eq_low = lo; tr.eq_mid = mid; tr.eq_high = hi;
                    tr.eq_mid_f = mid_f; tr.eq_mid_q = mid_q;
                    tr.hpf_on = hpf_on; tr.lpf_on = lpf_on; tr.hpf_hz = hpf_hz; tr.lpf_hz = lpf_hz;
                }
            });
        }
        if let Some((t, dev)) = input_edit {
            shared.edit(|st| {
                if let Some(tr) = st.tracks.get_mut(t) {
                    tr.input_device = dev;
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
    /// Escanea los plugins del formato activo en un hilo aparte (cargar bundles puede tardar
    /// segundos y no debe congelar la interfaz). El resultado se recoge en `poll_scan`.
    fn browser_rescan(&mut self) {
        if self.scan_rx.is_some() { return; }
        let mode = self.browser_mode;
        let (tx, rx) = std::sync::mpsc::channel();
        self.scan_rx = Some(rx);
        std::thread::spawn(move || {
            let mut kinds = std::collections::HashMap::new();
            let list: Vec<(String, String, String)> = match mode {
                BrowserMode::Clap => lanzadera_engine::clap_scan_kinds().into_iter()
                    .map(|(r, k)| { kinds.insert((r.0.clone(), r.1.clone()), k); r })
                    .collect(),
                // VST3: el id de CLAP no aplica; (path, "", nombre).
                BrowserMode::Vst3 => lanzadera_engine::vst3_scan_kinds().into_iter()
                    .map(|((p, n), k)| { kinds.insert((p.clone(), String::new()), k); (p, String::new(), n) })
                    .collect(),
            };
            let _ = tx.send((mode, list, kinds));
        });
    }

    fn poll_scan(&mut self) {
        let Some(rx) = &self.scan_rx else { return };
        match rx.try_recv() {
            Ok((mode, list, kinds)) => {
                self.scan_rx = None;
                if mode == self.browser_mode {
                    self.browser_kinds = kinds;
                    self.browser_list = Some(list);
                } else {
                    // Cambió de pestaña mientras escaneaba: se vuelve a escanear la actual.
                    self.browser_list = None;
                }
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.scan_rx = None;
                self.browser_list = Some(Vec::new());
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                if let Some(c) = &self.ctx_cache { c.request_repaint_after(std::time::Duration::from_millis(100)); }
            }
        }
    }

    // ── Fase 3: Preferencias ───────────────────────────────────────────────
    fn spawn_folder_dialog(&mut self, fmt: BrowserMode) {
        if self.folder_rx.is_some() { return; }
        let (tx, rx) = std::sync::mpsc::channel();
        self.folder_rx = Some(rx);
        std::thread::spawn(move || {
            let res = rfd::FileDialog::new().set_title("Elige la carpeta de plugins").pick_folder();
            let _ = tx.send((fmt, res));
        });
    }

    fn poll_folder_dialog(&mut self) {
        let Some(rx) = &self.folder_rx else { return };
        match rx.try_recv() {
            Ok((fmt, Some(path))) => {
                self.folder_rx = None;
                let p = path.to_string_lossy().into_owned();
                lanzadera_core::config::update(|c| {
                    let list = if fmt == BrowserMode::Clap { &mut c.clap_paths } else { &mut c.vst3_paths };
                    if !list.contains(&p) { list.push(p.clone()); }
                });
                self.browser_list = None;
                self.msg = format!("Carpeta de plugins añadida: {}", p);
            }
            Ok((_, None)) => { self.folder_rx = None; }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => { self.folder_rx = None; }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }

    fn prefs_window(&mut self, ctx: &egui::Context) {
        if !self.prefs_open { return; }
        let mut open = true;
        egui::Window::new("Preferencias").open(&mut open).default_size([580.0, 440.0]).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.prefs_tab, PrefsTab::Plugins, "Plugins");
                ui.selectable_value(&mut self.prefs_tab, PrefsTab::Midi, "Controladores MIDI");
                ui.selectable_value(&mut self.prefs_tab, PrefsTab::Audio, "Audio");
                ui.selectable_value(&mut self.prefs_tab, PrefsTab::Apariencia, "Apariencia");
            });
            ui.separator();
            match self.prefs_tab {
                PrefsTab::Plugins => self.prefs_plugins(ui),
                PrefsTab::Midi => self.prefs_midi(ui, ctx),
                PrefsTab::Audio => self.prefs_audio(ui),
                PrefsTab::Apariencia => self.prefs_look(ui, ctx),
            }
        });
        self.prefs_open = open;
    }

    fn prefs_plugins(&mut self, ui: &mut egui::Ui) {
        fn dirs_text(v: &[std::path::PathBuf]) -> String {
            v.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("  ·  ")
        }
        let mut cfg = lanzadera_core::config::get();
        let mut changed = false;
        changed |= ui.checkbox(&mut cfg.use_default_plugin_paths, "Buscar también en las carpetas estándar del sistema").changed();
        if cfg.use_default_plugin_paths {
            ui.label(egui::RichText::new(format!("CLAP: {}", dirs_text(&lanzadera_engine::clap_standard_dirs()))).size(10.0).color(T.text_dim));
            ui.label(egui::RichText::new(format!("VST3: {}", dirs_text(&lanzadera_engine::vst3_standard_dirs()))).size(10.0).color(T.text_dim));
        }
        for (fmt, title) in [(BrowserMode::Clap, "Carpetas CLAP añadidas"), (BrowserMode::Vst3, "Carpetas VST3 añadidas")] {
            ui.add_space(8.0);
            ui.label(egui::RichText::new(title).strong());
            let list = if fmt == BrowserMode::Clap { &mut cfg.clap_paths } else { &mut cfg.vst3_paths };
            let mut remove: Option<usize> = None;
            if list.is_empty() {
                ui.label(egui::RichText::new("(ninguna)").color(T.text_dim));
            }
            for (i, p) in list.iter().enumerate() {
                ui.horizontal(|ui| {
                    if ui.small_button("✕").on_hover_text("Quitar carpeta").clicked() { remove = Some(i); }
                    if std::path::Path::new(p).is_dir() {
                        ui.label(p.as_str());
                    } else {
                        ui.label(egui::RichText::new(format!("{} (no existe)", p)).color(egui::Color32::from_rgb(232, 100, 100)));
                    }
                });
            }
            if let Some(i) = remove { list.remove(i); changed = true; }
            if ui.button("＋ Añadir carpeta…").clicked() { self.spawn_folder_dialog(fmt); }
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button("Restaurar por defecto").on_hover_text("Quita las carpetas añadidas y vuelve a las estándar").clicked() {
                cfg.use_default_plugin_paths = true;
                cfg.clap_paths.clear();
                cfg.vst3_paths.clear();
                changed = true;
            }
            if ui.button("⟳ Re-escanear plugins").clicked() { self.browser_list = None; }
            if self.scan_rx.is_some() { ui.label("⏳ Escaneando…"); }
        });
        if changed {
            lanzadera_core::config::update(|c| {
                c.use_default_plugin_paths = cfg.use_default_plugin_paths;
                c.clap_paths = cfg.clap_paths.clone();
                c.vst3_paths = cfg.vst3_paths.clone();
            });
            self.browser_list = None; // la lista se vuelve a escanear con las carpetas nuevas
        }
    }

    fn prefs_midi(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        use lanzadera_engine::midi_ctl::{self, MidiProfile, MidiRequest};
        // Mientras esta pestaña está abierta se vuelve a listar cada 2 s (enchufar y ver).
        if self.now - self.midi_last_poll > 2.0 {
            self.midi_last_poll = self.now;
            midi_ctl::request(MidiRequest::Rescan);
        }
        ctx.request_repaint_after(std::time::Duration::from_secs(2));
        ui.horizontal(|ui| {
            if ui.button("🔍 Buscar controladores").clicked() {
                self.midi_last_poll = self.now;
                midi_ctl::request(MidiRequest::Rescan);
            }
            ui.label(egui::RichText::new("Conecta un teclado o pads y pulsa Buscar").size(10.0).color(T.text_dim));
        });
        let st = midi_ctl::status();
        ui.add_space(6.0);
        if st.ports.is_empty() {
            ui.label(egui::RichText::new("No se detectaron puertos MIDI de entrada.").color(T.text_dim));
        }
        let auto = lanzadera_core::config::get().midi_auto;
        for port in &st.ports {
            let lower = port.to_lowercase();
            let conn = st.connected.iter().find(|(prof, p)| {
                p == port || (*prof == MidiProfile::KeyLab && lower.contains("keylab"))
            }).map(|(prof, _)| *prof);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(port).strong());
                match conn {
                    Some(prof) => {
                        ui.label(egui::RichText::new(format!("● {}", prof.label())).color(T.accent));
                        if ui.small_button("Desconectar").clicked() {
                            midi_ctl::request(MidiRequest::Disconnect { profile: prof, port: port.clone() });
                        }
                        let mut is_auto = auto.iter().any(|a| a.profile == prof.id() && &a.port == port);
                        if ui.checkbox(&mut is_auto, "Conectar al iniciar").changed() {
                            let (id, pn) = (prof.id().to_string(), port.clone());
                            lanzadera_core::config::update(|c| {
                                c.midi_auto.retain(|a| !(a.profile == id && a.port == pn));
                                if is_auto { c.midi_auto.push(lanzadera_core::config::MidiAuto { profile: id.clone(), port: pn.clone() }); }
                            });
                        }
                    }
                    None => {
                        ui.label("Conectar como:");
                        if ui.small_button("Teclado / controlador").clicked() {
                            midi_ctl::request(MidiRequest::Connect { profile: MidiProfile::Generic, port: port.clone() });
                        }
                        if lower.contains("maschine") && ui.small_button("Maschine Mikro MK3").clicked() {
                            midi_ctl::request(MidiRequest::Connect { profile: MidiProfile::Mk3, port: port.clone() });
                        }
                        if lower.contains("keylab") && ui.small_button("KeyLab Essential 49").clicked() {
                            midi_ctl::request(MidiRequest::Connect { profile: MidiProfile::KeyLab, port: port.clone() });
                        }
                    }
                }
            });
        }
        if !st.msg.is_empty() {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(&st.msg).color(T.accent).size(11.0));
        }
    }

    fn prefs_audio(&mut self, ui: &mut egui::Ui) {
        let cfg = lanzadera_core::config::get();
        ui.label(egui::RichText::new("Latencia de grabación de audio").strong());
        ui.label(egui::RichText::new("Compensa el retraso de ida y vuelta de la interfaz al grabar. Si el loop grabado suena adelantado, baja el valor; si suena atrasado, súbelo. Se aplica a la próxima grabación.")
            .size(10.0).color(T.text_dim));
        let mut auto = cfg.rec_latency_ms.is_none();
        let mut ms = cfg.rec_latency_ms.unwrap_or(5.0);
        let a = ui.checkbox(&mut auto, "Automática (1 buffer de audio)").changed();
        let r = ui.add_enabled(!auto, egui::Slider::new(&mut ms, 0.0..=40.0).step_by(0.5).suffix(" ms"));
        if a || r.changed() {
            lanzadera_core::config::update(|c| c.rec_latency_ms = if auto { None } else { Some(ms) });
        }
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Cada pista elige su canal de entrada (CH) en el mixer. El monitoreo suena solo con la pista armada (R).")
            .size(10.0).color(T.text_dim));
    }

    fn prefs_look(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.label(egui::RichText::new("Tamaño de la interfaz").strong());
        let r = ui.add(egui::Slider::new(&mut self.ui_scale_edit, 0.75..=1.6).step_by(0.05).suffix("×"));
        if r.drag_stopped() || (r.changed() && !r.dragged()) {
            ctx.set_zoom_factor(self.ui_scale_edit);
            let v = self.ui_scale_edit;
            lanzadera_core::config::update(|c| c.ui_scale = v);
        }
        if ui.button("Restablecer (1.0×)").clicked() {
            self.ui_scale_edit = 1.0;
            ctx.set_zoom_factor(1.0);
            lanzadera_core::config::update(|c| c.ui_scale = 1.0);
        }
        ui.label(egui::RichText::new("Se aplica al soltar el control y se recuerda al reiniciar.").size(10.0).color(T.text_dim));
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
        let apply_layout = self.window_layout_apply_frames > 0;
        let default_sz = egui::vec2(980.0, 560.0);
        let saved_sz = self.windows.clip_launch_size.filter(|s| s.x > 100.0 && s.y > 100.0);
        let mut win = egui::Window::new("🎹 Clip Launch")
            .open(&mut open)
            .min_size([320.0, 200.0])
            .max_size([8000.0, 8000.0])
            .constrain(false)
            .resizable(true)
            .frame(Self::window_frame());
        if apply_layout {
            if let Some(s) = saved_sz { win = win.fixed_size(s); }
            else { win = win.default_size(default_sz); }
            if let Some(p) = self.windows.clip_launch_pos { win = win.current_pos(p); }
        } else {
            win = win.default_size(saved_sz.unwrap_or(default_sz));
            if let Some(p) = self.windows.clip_launch_pos { win = win.default_pos(p); }
        }
        let resp = win.show(ctx, |ui| {
                // Barra de herramientas del grid: +/− pistas
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("SESSION").size(11.0).strong().color(T.accent_gold));
                    ui.separator();
                    if ui.button("+ Pista").on_hover_text("Añadir pista").clicked() {
                        shared.edit(|st| {
                            let mut tr = lanzadera_core::Track::default();
                            tr.name = format!("Pista {}", st.tracks.len() + 1);
                            tr.kind = lanzadera_core::TrackKind::Audio;
                            let _ = st.push_track(tr);
                        });
                    }
                    let n = shared.state.lock().unwrap().tracks.len();
                    if n > 1 {
                        if ui.button("− Pista").on_hover_text("Eliminar pista seleccionada").clicked() {
                            shared.edit(|st| {
                                let idx = st.selected_track.min(st.tracks.len().saturating_sub(1));
                                if st.tracks.len() > 1 {
                                    st.tracks.remove(idx);
                                    if st.selected_track >= st.tracks.len() {
                                        st.selected_track = st.tracks.len().saturating_sub(1);
                                    }
                                }
                            });
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new(format!("{} × scenes", n)).size(10.0).color(T.text_dim));
                    });
                });
                ui.add_space(2.0);
                let sz = ui.available_size();
                ui.allocate_ui_with_layout(
                    sz, egui::Layout::top_down(egui::Align::LEFT),
                    |ui| { self.clip_grid(ui, ctx, shared, audio_on); },
                );
            });
        if resp.is_some() {
            let rect = ctx.memory(|mem| mem.area_rect(egui::Id::new("🎹 Clip Launch")));
            if let Some(r) = rect {
                self.windows.clip_launch_pos = Some(r.min);
                if !apply_layout || self.window_layout_apply_frames == 1 {
                    self.windows.clip_launch_size = Some(r.size());
                }
            }
        }
        self.windows.clip_launch_visible = open;
    }

    fn groove_machine_window(&mut self, ctx: &egui::Context, shared: &Arc<SessionShared>) {
        let mut open = self.windows.groove_machine_visible;
        let apply_layout = self.window_layout_apply_frames > 0;

        let screen = ctx.screen_rect();
        // Dejar margen para la barra superior de Lanzadera y ocupar casi toda la pantalla.
        let top_bar = screen.min.y + 56.0;
        let default_sz = egui::vec2(
            (screen.width() - 16.0).clamp(900.0, 2400.0),
            (screen.max.y - top_bar - 8.0).clamp(560.0, 1600.0),
        );
        let default_pos = egui::pos2(screen.min.x + 8.0, top_bar);
        let mut win = egui::Window::new("🥁 Groove Machine")
            .open(&mut open)
            .constrain(false)
            .max_size([8000.0, 8000.0])
            .min_size([560.0, 360.0])
            .resizable(true)
            .frame(Self::window_frame());
        let saved_sz = self.windows.groove_size.filter(|s| s.x > 100.0 && s.y > 100.0);
        let area_id = egui::Id::new("🥁 Groove Machine");
        let area_rect = ctx.memory(|mem| mem.area_rect(area_id));
        let has_good_state = area_rect.map(|r| r.width() >= 100.0 && r.height() >= 100.0).unwrap_or(false);
        if apply_layout {
            let sz = saved_sz.unwrap_or(default_sz);
            let pos = self.windows.groove_pos.unwrap_or(default_pos);
            win = win.fixed_size(sz).current_pos(pos);
        } else if !has_good_state {
            // Primera apertura / estado inválido: ocupar pantalla, respetando barra superior.
            win = win.default_size(saved_sz.unwrap_or(default_sz))
                .current_pos(self.windows.groove_pos.unwrap_or(default_pos));
        }
        // Con estado válido y sin apply_layout: no forzar tamaño/posición;
        // egui preserva el resize manual del usuario.
        let resp = win.show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.groove_tab, GrooveScreenTab::Main, "Groove");
                    ui.selectable_value(&mut self.groove_tab, GrooveScreenTab::Step, "Step");

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

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (gmaster, gpeak) = {
                            let st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                            (st.groove_master, shared.rt.groove_peak())
                        };
                        let mut gm = gmaster;
                        ui.add_space(4.0);
                        let r = ui.add(egui::Slider::new(&mut gm, 0.0..=1.5).text("GM Master").show_value(false));
                        if r.changed() { shared.apply_control(ControlCmd::SetGrooveMasterVol { value: gm }); }
                        ui.label(egui::RichText::new(format!("OUT {:>4.1} dB", 20.0 * (gpeak.max(1e-5)).log10())).size(10.0).color(T.text_dim));
                    });
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
                    if ui.button("📤 A Clip Launch")
                        .on_hover_text("Exportar el patrón Groove actual como MIDI a la escena seleccionada")
                        .clicked() {
                        let scene = shared.state.lock().unwrap_or_else(|e| e.into_inner()).selected_scene;
                        self.export_groove_to_clip_launch(shared, scene);
                    }
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

                // Submodo lógico para MK3/LEDs: 0 Pad, 1 Step, 2 Mixer, 3 Chords.
                let sub_mode_val: u8 = if self.groove_chords { 3 } else { match self.groove_tab {
                    GrooveScreenTab::Step => 1,
                    GrooveScreenTab::Main => match self.groove_main_focus {
                        GrooveMainFocus::Detail if self.groove_detail_tab == GrooveDetailTab::Params => 2,
                        _ => 0,
                    },
                }};
                shared.groove_sub_mode.store(sub_mode_val, std::sync::atomic::Ordering::Relaxed);
                shared.mk3_chords_mode.store(self.groove_chords, std::sync::atomic::Ordering::Relaxed);

                match self.groove_tab {
                    GrooveScreenTab::Main => {
                        self.groove_main_tab(ui, shared);
                    }
                    GrooveScreenTab::Step => {
                        self.inspector_focus = InspectorFocus::GrooveStep;
                        self.groove_step_tab(ui, shared);
                    }
                }
            });

        if resp.is_some() {
            let area_id = egui::Id::new("🥁 Groove Machine");
            let rect = ctx.memory(|mem| mem.area_rect(area_id));
            if let Some(r) = rect {
                self.windows.groove_pos = Some(r.min);
                // Persistir tamaño real siempre que no estemos en frames de fixed_size de apply.
                if !apply_layout || self.window_layout_apply_frames == 1 {
                    self.windows.groove_size = Some(r.size());
                }
            }
        }
        // Si el usuario cierra la ventana con la X, apagamos el motor.
        if !open {
            shared.caja_sched.enabled.store(false, std::sync::atomic::Ordering::Relaxed);
        }

        self.windows.groove_machine_visible = open;
    }

    fn export_groove_to_clip_launch(&mut self, shared: &SessionShared, scene: usize) {
        // Exporta Groove a MIDI temporal y lo importa como clip en una pista Groove dedicada.
        let mid_path = match shared.groove.export_midi(false, true) {
            Ok(p) => p,
            Err(e) => { self.msg = format!("⚠ No se pudo exportar Groove: {}", e); return; }
        };
        let (mut notes, bars) = match lanzadera_engine::midi_file::parse_midi(&mid_path.to_string_lossy()) {
            Ok(n) => n,
            Err(e) => { self.msg = format!("⚠ No se pudo leer MIDI de Groove: {}", e); return; }
        };
        if notes.is_empty() {
            self.msg = "⚠ El patrón Groove no tiene notas para exportar".into();
            return;
        }
        let loop_bars = bars.max(self.groove_snap.as_ref().map(|s| s.pattern.bars.max(1) as u8).unwrap_or(4));
        notes.sort_by_key(|n| (n.tick_start, n.note));

        let (track, sc) = shared.edit(|st| {
            let idx = if let Some(i) = st.tracks.iter().position(|t| t.name == "Groove") { i } else {
                let mut tr = lanzadera_core::Track::default();
                tr.name = "Groove".into();
                tr.kind = lanzadera_core::TrackKind::Instrument;
                tr.color = [240, 170, 60];
                let _ = st.push_track(tr);
                st.tracks.len().saturating_sub(1)
            };
            let mut sc = scene;
            if let Some(tr) = st.tracks.get_mut(idx) {
                sc = scene.min(tr.clips.len().saturating_sub(1));
                let mut c = lanzadera_core::Clip::default();
                c.name = format!("Groove S{}", sc + 1);
                c.loop_bars = loop_bars;
                c.midi_notes = notes;
                c.color = [240, 170, 60];
                if let Some(slot) = tr.clips.get_mut(sc) { *slot = Some(c); }
            }
            st.selected_track = idx;
            st.selected_scene = sc;
            st.selected_clip = Some((idx, sc));
            (idx, sc)
        });
        shared.sync_midi(track, sc);
        shared.samples.clear(track, sc);
        self.inspector_focus = InspectorFocus::Clip;
        self.msg = format!("Groove exportado a Clip Launch: P{}·E{}", track + 1, sc + 1);
    }

    fn groove_main_tab(&mut self, ui: &mut egui::Ui, shared: &Arc<SessionShared>) {
        ui.vertical(|ui| {
            self.groove_header_bar(ui, shared);
            ui.add_space(4.0);
            ui.separator();
            self.groove_source_strip(ui, shared);
            ui.add_space(4.0);
            ui.separator();

            let avail = ui.available_height().max(200.0);
            let rack_h = (avail * 0.24).clamp(110.0, 180.0);
            let top_h = (avail - rack_h - 8.0).max(160.0);

            ui.allocate_ui_with_layout(vec2(ui.available_width(), top_h), egui::Layout::left_to_right(egui::Align::TOP), |ui| {
                let total_w = ui.available_width().max(1.0);
                let gap = 8.0;
                // Pads amplio, editor sample compacto (casi cuadrado), SAMPLES/PARAMS con protagonismo.
                let left_w = (total_w * 0.40).clamp(300.0, 520.0);
                let right_w = (total_w * 0.34).clamp(320.0, 500.0);
                let mid_w = (total_w - left_w - right_w - gap * 2.0).clamp(180.0, 280.0);

                ui.allocate_ui_with_layout(vec2(left_w, top_h), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                    ui.set_max_width(left_w);
                    egui::ScrollArea::vertical().id_source("gm_main_pads_scroll").auto_shrink([false, false]).show(ui, |ui| {
                        self.groove_pad_grid_unified(ui, shared);
                    });
                });
                ui.add_space(gap);
                ui.allocate_ui_with_layout(vec2(mid_w, top_h), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                    ui.set_max_width(mid_w);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("VISTA").size(9.0).color(T.text_dim));
                        if ui.selectable_label(self.groove_bank == 0, "A").clicked() { self.groove_bank = 0; }
                        if ui.selectable_label(self.groove_bank == 1, "B").clicked() { self.groove_bank = 1; }
                        ui.separator();
                        if ui.selectable_label(self.groove_tab == GrooveScreenTab::Main, "Rack").clicked() {
                            self.groove_tab = GrooveScreenTab::Main;
                            self.groove_main_focus = GrooveMainFocus::Rack;
                        }
                        if ui.button("Step").clicked() {
                            self.groove_tab = GrooveScreenTab::Step;
                            self.groove_main_focus = GrooveMainFocus::Rack;
                        }
                    });
                    ui.add_space(4.0);
                    egui::Frame::none()
                        .fill(Color32::from_rgb(34, 31, 28))
                        .rounding(Rounding::same(6.0))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(72, 66, 58)))
                        .inner_margin(8.0)
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical().id_source("gm_main_sample_scroll").auto_shrink([false, false]).show(ui, |ui| {
                                self.groove_embedded_sample_editor(ui, shared);
                            });
                        });
                });
                ui.add_space(gap);
                ui.allocate_ui_with_layout(vec2(right_w, top_h), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                    ui.set_max_width(right_w);
                    egui::Frame::none().fill(Color32::from_rgb(34, 31, 28)).rounding(Rounding::same(6.0)).stroke(Stroke::new(1.0, T.accent_gold.gamma_multiply(0.35))).inner_margin(8.0).show(ui, |ui| {
                        egui::ScrollArea::vertical().id_source("gm_main_detail_scroll").auto_shrink([false, false]).show(ui, |ui| {
                            self.groove_pad_detail_panel(ui, shared);
                        });
                    });
                });
            });

            ui.add_space(4.0);
            ui.separator();
            ui.allocate_ui_with_layout(vec2(ui.available_width(), rack_h), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                self.groove_rack_strip(ui, shared);
            });
        });
    }

    fn groove_header_bar(&mut self, ui: &mut egui::Ui, shared: &Arc<SessionShared>) {
        let frame = egui::Frame::none()
            .fill(Color32::from_rgb(43, 39, 35))
            .stroke(Stroke::new(1.0, Color32::from_rgb(78, 70, 60)))
            .rounding(Rounding::same(6.0))
            .inner_margin(egui::Margin::symmetric(10.0, 8.0));
        frame.show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new("GROOVE SAMPLER").size(11.0).color(T.accent_gold).strong().monospace());
                ui.separator();
                ui.label(egui::RichText::new("PRESET").size(9.0).color(T.text_dim).strong());
                let presets = shared.list_groove_presets();
                egui::ComboBox::from_id_source("gm_preset_combo_main")
                    .selected_text(if self.groove_preset_selected.is_empty() { "elegir…".to_string() } else { self.groove_preset_selected.clone() })
                    .width(170.0)
                    .show_ui(ui, |ui| {
                        for p in &presets {
                            ui.selectable_value(&mut self.groove_preset_selected, p.clone(), p.as_str());
                        }
                    });
                if ui.small_button("Cargar").clicked() && !self.groove_preset_selected.is_empty() {
                    match shared.load_groove_preset(&self.groove_preset_selected) {
                        Ok(()) => self.msg = format!("Preset cargado: {}", self.groove_preset_selected),
                        Err(e) => self.msg = e,
                    }
                }
                ui.add(egui::TextEdit::singleline(&mut self.groove_preset_name).desired_width(110.0).hint_text("nombre…"));
                if ui.small_button("Guardar").clicked() {
                    if self.groove_preset_name.trim().is_empty() {
                        self.groove_preset_name = format!("Groove {}", presets.len() + 1);
                    }
                    match shared.save_groove_preset(&self.groove_preset_name) {
                        Ok(()) => { self.groove_preset_selected = self.groove_preset_name.trim().to_string(); self.msg = format!("Preset guardado: {}", self.groove_preset_selected); }
                        Err(e) => self.msg = e,
                    }
                }
                if ui.small_button("Export").clicked() {
                    let n = self.groove_preset_name.trim().to_string();
                    if n.is_empty() { self.msg = "Escribe un nombre para exportar.".into(); }
                    else { let path = format!("{}.groovepreset", n.replace('/', "_")); match shared.export_groove_preset(&path) { Ok(()) => self.msg = format!("Exportado {}", path), Err(e) => self.msg = e } }
                }
                if ui.small_button("Import").clicked() {
                    let n = self.groove_preset_name.trim().to_string();
                    if n.is_empty() { self.msg = "Escribe el nombre del archivo a importar (sin extensión).".into(); }
                    else { let path = format!("{}.groovepreset", n.replace('/', "_")); match shared.import_groove_preset(&path, &n) { Ok(()) => { self.groove_preset_selected = n; self.msg = format!("Importado {}", path); } Err(e) => self.msg = e } }
                }
            });

            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                let enabled = shared.caja_sched.enabled.load(std::sync::atomic::Ordering::Relaxed);
                let motor = ui.add(egui::Button::new(egui::RichText::new(if enabled { "● MOTOR ON" } else { "○ MOTOR OFF" }).size(11.0).strong())
                    .fill(if enabled { Color32::from_rgb(52, 88, 58) } else { Color32::from_rgb(56, 50, 44) })
                    .stroke(Stroke::new(1.0, if enabled { T.accent_green } else { Color32::from_rgb(90, 80, 68) })));
                if motor.clicked() { shared.caja_sched.toggle(); }

                let step = groove_head(shared);
                ui.add_space(6.0);
                ui.label(egui::RichText::new(if enabled && step != 255 { format!("STEP {:02}/16", step + 1) } else { "STEP --/16".into() }).size(10.0).color(if enabled { T.accent_green } else { T.text_dim }).monospace());
                let (rect, _) = ui.allocate_exact_size(vec2(170.0, 14.0), Sense::hover());
                let p = ui.painter_at(rect);
                p.rect_filled(rect, 3.0, Color32::from_rgb(22, 20, 18));
                let cw = rect.width() / 16.0;
                for i in 0..16 {
                    let r = Rect::from_min_size(pos2(rect.left() + i as f32 * cw + 1.0, rect.top() + 2.0), vec2(cw - 2.0, rect.height() - 4.0));
                    let on = enabled && i as u8 == step;
                    let beat = i % 4 == 0;
                    p.rect_filled(r, 1.0, if on { T.accent_green } else if beat { Color32::from_rgb(70, 64, 56) } else { Color32::from_rgb(48, 44, 40) });
                }

                ui.separator();
                let (mut gmaster, gpeak) = {
                    let st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                    (st.groove_master, shared.rt.groove_peak())
                };
                ui.label(egui::RichText::new("OUT").size(9.0).color(T.accent_gold).strong());
                let meter_rect = ui.allocate_exact_size(vec2(100.0, 12.0), Sense::hover()).0;
                let mp = ui.painter_at(meter_rect);
                mp.rect_filled(meter_rect, 2.0, Color32::from_rgb(20, 18, 16));
                let lvl = gpeak.clamp(0.0, 1.0).sqrt();
                let fill = Rect::from_min_size(meter_rect.min, vec2(meter_rect.width() * lvl, meter_rect.height()));
                mp.rect_filled(fill, 2.0, if lvl > 0.9 { T.meter_high } else if lvl > 0.7 { T.meter_mid } else { T.accent_green });
                ui.label(egui::RichText::new(format!("{:>5.1} dB", 20.0 * gpeak.max(1e-5).log10())).size(9.0).color(T.text_dim).monospace());
                let r = ui.add(egui::Slider::new(&mut gmaster, 0.0..=1.5).text("GM").show_value(false));
                if r.changed() { shared.apply_control(ControlCmd::SetGrooveMasterVol { value: gmaster }); }
            });
        });
    }

    fn groove_source_strip(&mut self, ui: &mut egui::Ui, shared: &Arc<SessionShared>) {
        let (mut p, genre_name, reading, genre_names) = match &self.groove_snap {
            Some(s) => (s.p, s.genre_name.clone(), s.reading.clone(), s.genre_names.clone()),
            None => { ui.label(egui::RichText::new("Cargando Groove Machine…").color(T.text_dim)); return; }
        };
        let kit = shared.kit.clone();
        let sel_pad = self.groove_sel_pad.min(24);
        let cur_rate = kit.snapshot_cfg().get(sel_pad).map(|c| c.rate).unwrap_or(4);

        egui::Frame::none()
            .fill(Color32::from_rgb(40, 36, 32))
            .stroke(Stroke::new(1.0, Color32::from_rgb(74, 67, 58)))
            .rounding(Rounding::same(6.0))
            .inner_margin(egui::Margin::symmetric(10.0, 8.0))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new("SOURCE").size(9.0).color(T.accent_gold).strong());
                    if ui.small_button("◀").clicked() { shared.groove.cycle_genre(-1); }
                    egui::ComboBox::from_id_source("gm_genre_main").selected_text(&genre_name).width(150.0).show_ui(ui, |ui| {
                        for (i, n) in genre_names.iter().enumerate() { if ui.selectable_label(n == &genre_name, n).clicked() { shared.groove.set_genre(i); } }
                    });
                    if ui.small_button("▶").clicked() { shared.groove.cycle_genre(1); }
                    ui.add(egui::TextEdit::singleline(&mut self.groove_phrase).desired_width(240.0).hint_text("frase / idea…"));
                    if ui.add(egui::Button::new("Generar").fill(Color32::from_rgb(74, 64, 50))).clicked() { shared.groove.generate(&self.groove_phrase); }
                    if ui.button("↻ Regenerar").clicked() { shared.groove.reroll(true); }
                });

                ui.add_space(2.0);
                ui.label(egui::RichText::new(&reading).size(10.0).color(Color32::from_rgb(188, 176, 158)));

                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    fn gm_knob(ui: &mut egui::Ui, label: &str, v: &mut i32, suffix: &str) -> bool {
                        let mut changed = false;
                        ui.vertical(|ui| {
                            ui.set_width(72.0);
                            ui.label(egui::RichText::new(label).size(9.0).color(T.text_dim));
                            let mut f = *v as f32;
                            let r = ui.add(egui::Slider::new(&mut f, 0.0..=100.0).show_value(false));
                            if r.changed() { *v = f.round().clamp(0.0, 100.0) as i32; changed = true; }
                            ui.label(egui::RichText::new(format!("{}{}", *v, suffix)).size(9.0).color(T.text_bright).monospace());
                        });
                        changed
                    }
                    if gm_knob(ui, "SWING", &mut p.swing, "%") { shared.groove.set_swing(p.swing); }
                    if gm_knob(ui, "ACCENT", &mut p.accent, "%") { shared.groove.set_accent(p.accent); }
                    if gm_knob(ui, "HUMAN", &mut p.human, "%") { shared.groove.set_human(p.human); }
                    if gm_knob(ui, "DENSITY", &mut p.density, "%") { shared.groove.set_density(p.density); }

                    ui.separator();
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("BARS").size(9.0).color(T.text_dim));
                        const GM_BARS_LOCAL: [&str; 4] = ["1", "2", "4", "8"];
                        let bars_idx = p.bars_idx.min(3) as usize;
                        egui::ComboBox::from_id_source("gm_bars_main").selected_text(GM_BARS_LOCAL[bars_idx]).width(48.0).show_ui(ui, |ui| {
                            for (i, lbl) in GM_BARS_LOCAL.iter().enumerate() { if ui.selectable_label(bars_idx == i, *lbl).clicked() { shared.groove.set_bars_idx(i as i32); } }
                        });
                    });
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("SEED").size(9.0).color(T.text_dim));
                        ui.horizontal(|ui| {
                            let mut seed = p.seed as i32;
                            if ui.add(egui::DragValue::new(&mut seed).range(0..=9999)).changed() { shared.groove.set_seed(seed.clamp(0, 9999)); }
                            if ui.small_button("🎲").clicked() { shared.groove.randomize_seed(); }
                        });
                    });
                });

                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new("CAMBIO MUESTRA").size(9.0).color(T.accent_gold).strong());
                    for (bars, lbl) in [(0u8, "Al toque"), (1, "1c"), (2, "2c"), (3, "3c"), (4, "4c")] {
                        let on = cur_rate == bars;
                        if ui.add(egui::Button::new(egui::RichText::new(lbl).size(10.0)).fill(if on { T.accent_gold.gamma_multiply(0.85) } else { Color32::from_rgb(52, 48, 42) })).clicked() {
                            kit.set_rate(sel_pad, bars);
                        }
                    }
                    ui.separator();
                    ui.label(egui::RichText::new("FX GLOBAL").size(9.0).color(T.text_dim).strong());
                    let (mut fx_pitch, mut fx_filter, mut fx_perform) = {
                        let st = shared.state.lock().unwrap();
                        (st.groove_fx_pitch, st.groove_fx_filter, st.groove_fx_perform)
                    };
                    let mut cmds: Vec<ControlCmd> = Vec::new();
                    ui.label(egui::RichText::new("PITCH").size(9.0).color(T.text_dim));
                    let r = ui.add(egui::Slider::new(&mut fx_pitch, 0.0..=1.0).show_value(false));
                    if r.changed() { cmds.push(ControlCmd::SetGrooveFxPitch { value: fx_pitch }); }
                    if r.double_clicked() { cmds.push(ControlCmd::SetGrooveFxPitch { value: 0.5 }); }
                    ui.label(egui::RichText::new("MOD").size(9.0).color(T.text_dim));
                    let r = ui.add(egui::Slider::new(&mut fx_filter, 0.0..=1.0).show_value(false));
                    if r.changed() { cmds.push(ControlCmd::SetGrooveFxFilter { value: fx_filter }); }
                    if r.double_clicked() { cmds.push(ControlCmd::SetGrooveFxFilter { value: 0.5 }); }
                    ui.label(egui::RichText::new("PERFORM").size(9.0).color(T.text_dim));
                    let r = ui.add(egui::Slider::new(&mut fx_perform, 0.0..=1.0).show_value(false));
                    if r.changed() { cmds.push(ControlCmd::SetGrooveFxPerform { value: fx_perform }); }
                    if r.double_clicked() { cmds.push(ControlCmd::SetGrooveFxPerform { value: 0.0 }); }
                    for c in cmds { shared.apply_control(c); }
                });
            });
    }

    fn groove_pad_grid_unified(&mut self, ui: &mut egui::Ui, shared: &Arc<SessionShared>) {
        use caja_core::pieces::{PAD_ORDER, PIECES};
        let (active, filled, names) = match &self.groove_snap {
            Some(s) => (s.active, s.filled, s.names.clone()),
            None => (0, [false; 32], std::array::from_fn::<String, 32, _>(|_| String::new())),
        };
        let pattern_mode = shared.mk3_pattern_mode.load(std::sync::atomic::Ordering::Relaxed);
        let kit = shared.kit.clone();
        let cfg = kit.snapshot_cfg();
        let live = kit.snapshot_live();

        egui::Frame::none()
            .fill(Color32::from_rgb(36, 33, 30))
            .stroke(Stroke::new(1.0, Color32::from_rgb(72, 66, 58)))
            .rounding(Rounding::same(6.0))
            .inner_margin(egui::Margin::symmetric(10.0, 8.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("PADS").size(11.0).strong().color(T.accent_gold).monospace());
                    ui.label(egui::RichText::new(if pattern_mode { "PATTERN MODE" } else { "DRUM PADS" }).size(9.0).color(T.text_dim));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.selectable_label(pattern_mode, egui::RichText::new("PATTERN").size(9.0)).clicked() {
                            shared.mk3_pattern_mode.store(!pattern_mode, std::sync::atomic::Ordering::Relaxed);
                        }
                        ui.label(egui::RichText::new("RACK").size(9.0).color(T.text_dim));
                        if ui.selectable_label(self.groove_bank == 1, "B").clicked() { self.groove_bank = 1; }
                        if ui.selectable_label(self.groove_bank == 0, "A").clicked() { self.groove_bank = 0; }
                    });
                });
                ui.add_space(6.0);

                let grid_w = ui.available_width().max(240.0);
                let pad_w = ((grid_w - 24.0) / 4.0).clamp(58.0, 138.0);
                let pad_h = 78.0;
                egui::Grid::new("gm_pads_unified").num_columns(4).spacing(vec2(8.0, 8.0)).show(ui, |ui| {
                    for i in 0..16 {
                        let slot = self.groove_bank * 16 + i;
                        let selected = if pattern_mode { slot == active } else { self.groove_sel_pad == i };
                        let pc = PAD_ORDER[i];
                        let note = self.groove_snap.as_ref().map(|s| s.notes[pc]).unwrap_or(36 + i as u8);
                        let name = if pattern_mode {
                            let nm = &names[slot];
                            if nm.is_empty() { format!("{}{}", if slot < 16 { "A" } else { "B" }, slot % 16 + 1) } else { trunc(nm, 10) }
                        } else { trunc(PIECES[pc].name, 11) };

                        let (rect, resp) = ui.allocate_exact_size(vec2(pad_w, pad_h), Sense::click());
                        let p = ui.painter_at(rect);
                        let base = if pattern_mode && filled[slot] { Color32::from_rgb(52, 47, 40) } else { Color32::from_rgb(44, 40, 36) };
                        let bg = if selected { Color32::from_rgb(66, 58, 46) } else { base };
                        p.rect_filled(rect, 5.0, bg);
                        let border_col = if selected { T.accent_gold } else { Color32::from_rgb(84, 76, 66) };
                        p.rect_stroke(rect, 5.0, Stroke::new(if selected { 2.0 } else { 1.0 }, border_col));
                        let hue = PAD_PALETTE[i % PAD_PALETTE.len()];
                        p.rect_filled(Rect::from_min_size(rect.min + vec2(0.0, 4.0), vec2(4.0, rect.height() - 8.0)), 2.0, hue.gamma_multiply(0.85));

                        let wf_rect = Rect::from_min_max(pos2(rect.min.x + 10.0, rect.min.y + 22.0), pos2(rect.max.x - 8.0, rect.max.y - 20.0));
                        if !pattern_mode {
                            let peaks = self.groove_pad_wave_peaks(shared, i);
                            if peaks.is_empty() {
                                p.rect_stroke(wf_rect, 3.0, Stroke::new(1.0, Color32::from_rgb(72, 66, 58)));
                                p.text(wf_rect.center(), Align2::CENTER_CENTER, "+", FontId::proportional(18.0), T.accent_gold.gamma_multiply(0.35));
                            } else {
                                crate::waveform::paint(&p, wf_rect, &peaks, None, &crate::waveform::WaveStyle { bg: Color32::from_rgb(28, 26, 24), wave: T.accent_gold.gamma_multiply(0.95), center: Color32::from_rgb(72, 66, 58), head: T.accent_green });
                            }
                        } else {
                            let tag = if filled[slot] { "●" } else { "○" };
                            p.text(wf_rect.center(), Align2::CENTER_CENTER, tag, FontId::proportional(22.0), if filled[slot] { T.accent_gold } else { T.text_dim });
                        }

                        p.text(pos2(rect.min.x + 10.0, rect.min.y + 5.0), Align2::LEFT_TOP, &name, FontId::proportional(10.0), if selected { Color32::from_rgb(244, 236, 222) } else { Color32::from_rgb(196, 184, 166) });
                        let sub = if pattern_mode { format!("slot {}", slot + 1) } else { format!("n{}", note) };
                        p.text(pos2(rect.max.x - 8.0, rect.max.y - 5.0), Align2::RIGHT_BOTTOM, sub, FontId::proportional(9.0), T.text_dim);
                        if !pattern_mode {
                            let c = &cfg[i.min(cfg.len()-1)];
                            if c.mute { p.text(pos2(rect.max.x - 8.0, rect.min.y + 5.0), Align2::RIGHT_TOP, "M", FontId::proportional(9.0), T.clip_rec); }
                            else if c.solo { p.text(pos2(rect.max.x - 8.0, rect.min.y + 5.0), Align2::RIGHT_TOP, "S", FontId::proportional(9.0), T.accent_gold); }
                        }

                        if resp.clicked() {
                            self.groove_main_focus = GrooveMainFocus::Pads;
                            if pattern_mode { shared.groove.select_pattern(slot); }
                            else {
                                self.groove_sel_pad = i;
                                shared.groove_sel_pad.store(i as u8, std::sync::atomic::Ordering::Relaxed);
                            }
                        }
                        if resp.double_clicked() && !pattern_mode {
                            let cmd = ControlCmd::LivePadOn { pad: i as u8, vel: 100 };
                            let _ = shared.trig_rx.push(cmd.encode());
                        }
                        if (i + 1) % 4 == 0 { ui.end_row(); }
                    }
                });
            });

        let _ = (active, live);
    }

    fn groove_pad_wave_peaks(&mut self, shared: &Arc<SessionShared>, pad: usize) -> Vec<f32> {
        let pad = pad.min(24);
        let slot = shared.kit.snapshot_live().get(pad).map(|l| l.slot as usize).unwrap_or(0);
        if self.groove_wave_cache_pad[pad] == slot {
            if let Some(c) = &self.groove_wave_cache[pad] { return (**c).clone(); }
        }
        let peaks = shared.caja_store.load_sample(pad, slot)
            .map(|s| s.overview(96))
            .unwrap_or_default();
        self.groove_wave_cache_pad[pad] = slot;
        self.groove_wave_cache[pad] = Some(std::sync::Arc::new(peaks.clone()));
        peaks
    }

    fn groove_pad_detail_panel(&mut self, ui: &mut egui::Ui, shared: &Arc<SessionShared>) {
        use caja_core::pieces::{PAD_ORDER, PIECES};
        let pad = self.groove_sel_pad.min(24);
        let pc = PAD_ORDER[pad];
        let note = self.groove_snap.as_ref().map(|s| s.notes[pc]).unwrap_or(36 + pad as u8);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("DETALLE · {} {:02}", PIECES[pc].name, pad + 1)).size(11.0).strong().color(T.accent_gold));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.selectable_label(self.groove_detail_tab == GrooveDetailTab::Params, "PARAMS").clicked() {
                    self.groove_detail_tab = GrooveDetailTab::Params; self.groove_main_focus = GrooveMainFocus::Detail;
                    if !self.groove_chords { shared.groove_sub_mode.store(2, std::sync::atomic::Ordering::Relaxed); }
                }
                if ui.selectable_label(self.groove_detail_tab == GrooveDetailTab::Chord, "SAMPLES").clicked() {
                    self.groove_detail_tab = GrooveDetailTab::Chord; self.groove_main_focus = GrooveMainFocus::Detail;
                }
            });
        });
        ui.label(egui::RichText::new(format!("Pieza {} · nota MIDI {}", pc, note)).size(10.0).color(T.text_dim));
        ui.separator();

        match self.groove_detail_tab {
            GrooveDetailTab::Chord => {
                self.inspector_focus = InspectorFocus::GroovePad;
                self.inspector_groove_chords(ui, shared);
            }
            GrooveDetailTab::Params => {
                self.inspector_focus = InspectorFocus::GrooveMixer;
                self.inspector_groove_cell(ui, shared);
            }
        }
        ui.add_space(6.0);
        ui.small_button("Editar en Inspector").on_hover_text("El inspector global se conserva como acceso paralelo").clicked().then(|| {
            self.windows.inspector_visible = true;
            self.inspector_focus = if self.groove_detail_tab == GrooveDetailTab::Params { InspectorFocus::GrooveMixer } else { InspectorFocus::GroovePad };
        });
    }

    fn groove_embedded_sample_editor(&mut self, ui: &mut egui::Ui, shared: &Arc<SessionShared>) {
        use lanzadera_engine::sample_edit::SampleOp;
        use caja_core::pieces::{PAD_ORDER, PIECES};
        let pad = self.groove_sel_pad.min(24);
        let pc = PAD_ORDER[pad];
        let slot = shared.kit.snapshot_live().get(pad).map(|l| l.slot as usize).unwrap_or(0).min(7);
        if self.groove_sample_pad != pad || self.groove_sample_slot != slot {
            self.groove_sample_pad = pad;
            self.groove_sample_slot = slot;
            self.groove_sample_zoom = 1.0;
            self.groove_sample_scroll = 0.0;
            self.groove_sample_sel = None;
            self.groove_sample_msg.clear();
        }

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("EDITOR DE MUESTRA").size(11.0).strong().color(T.accent_gold));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(format!("{} · slot {}/8", PIECES[pc].name, slot + 1)).size(10.0).color(T.text_dim));
            });
        });
        ui.add_space(4.0);

        let sample = shared.caja_store.load_sample(pad, slot);
        let Some(sd) = sample else {
            ui.label(egui::RichText::new("No hay muestra cargada en este slot.").color(T.text_dim));
            ui.label(egui::RichText::new("Usa SAMPLES para asignar/cargar muestras en este pad.").size(10.0).color(T.text_dim));
            return;
        };

        let _ch = if sd.right.is_some() { 2usize } else { 1usize };
        let frames = sd.len();
        if frames == 0 { ui.label("Muestra vacía."); return; }
        // Los samples de caja no guardan sample_rate; asumimos 44100 para UI.
        let sample_rate = 44100.0f64;
        let secs = frames as f64 / sample_rate;

        let mut op: Option<SampleOp> = None;
        ui.horizontal_wrapped(|ui| {
            ui.label("Zoom");
            ui.add_sized(vec2(90.0, 18.0), egui::Slider::new(&mut self.groove_sample_zoom, 1.0..=64.0).logarithmic(true).show_value(false));
            if ui.button("Ver todo").clicked() { self.groove_sample_zoom = 1.0; self.groove_sample_scroll = 0.0; }
            ui.separator();
            ui.label("Gain");
            ui.add_sized(vec2(110.0, 18.0), egui::Slider::new(&mut self.groove_sample_gain_db, -24.0..=24.0).suffix(" dB"));
            if ui.button("Aplicar").clicked() { op = Some(SampleOp::Gain((10.0f32).powf(self.groove_sample_gain_db / 20.0))); }
            if ui.button("Norm").clicked() { op = Some(SampleOp::Normalize); }
            if ui.button("Rev").clicked() { op = Some(SampleOp::Reverse); }
        });

        let sel_range = self.groove_sample_sel.map(|(a,b)|(a.min(b), a.max(b))).filter(|(a,b)| b-a > 0.001);
        ui.horizontal_wrapped(|ui| {
            match sel_range {
                Some((lo, hi)) => {
                    ui.label(egui::RichText::new(format!("{:.2}-{:.2}s", lo * secs as f32, hi * secs as f32)).size(10.0).color(T.text_dim));
                    if ui.button("Silenciar").clicked() { op = Some(SampleOp::Silence(lo, hi)); }
                    if ui.button("Fade in").clicked() { op = Some(SampleOp::FadeIn(lo, hi)); }
                    if ui.button("Fade out").clicked() { op = Some(SampleOp::FadeOut(lo, hi)); }
                    if ui.button("Loop start").clicked() { self.groove_sample_msg = format!("Selección inicio: {:.3}s (loop clip solo en Clip Launch)", lo * secs as f32); }
                }
                None => { ui.label(egui::RichText::new("Arrastra sobre la onda para seleccionar").size(10.0).color(T.text_dim)); }
            }
        });

        let zoom = self.groove_sample_zoom.clamp(1.0, 64.0);
        let span = 1.0 / zoom;
        self.groove_sample_scroll = self.groove_sample_scroll.clamp(0.0, (1.0 - span).max(0.0));
        let a = self.groove_sample_scroll;
        let b = (a + span).min(1.0);
        // Más compacto/casi cuadrado: que el panel derecho tenga más protagonismo.
        let w = ui.available_width().max(80.0);
        let h = w.clamp(120.0, 220.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click_and_drag());
        if resp.hovered() {
            let dy = ui.input(|i| i.raw_scroll_delta.y);
            if dy.abs() > 0.5 {
                let px = resp.hover_pos().map(|p| ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0)).unwrap_or(0.5);
                let focus = a + px * span;
                let nz = (zoom * (1.0 + dy * 0.002)).clamp(1.0, 64.0);
                let nspan = 1.0 / nz;
                self.groove_sample_zoom = nz;
                self.groove_sample_scroll = (focus - px * nspan).clamp(0.0, (1.0 - nspan).max(0.0));
            }
        }
        let frac = |x: f32| (a + ((x - rect.left()) / rect.width()).clamp(0.0, 1.0) * span).clamp(0.0, 1.0);
        if resp.drag_started() {
            if let Some(p) = resp.interact_pointer_pos() { let f = frac(p.x); self.groove_sample_sel = Some((f, f)); }
        }
        if resp.dragged() {
            if let (Some(p), Some((s0, _))) = (resp.interact_pointer_pos(), self.groove_sample_sel) { self.groove_sample_sel = Some((s0, frac(p.x))); }
        }
        if resp.double_clicked() { self.groove_sample_sel = None; }

        let f0 = ((a * frames as f32) as usize).min(frames.saturating_sub(1));
        let f1 = (((b * frames as f32) as usize).max(f0 + 1)).min(frames);
        let cols = (rect.width() as usize).clamp(1, 2048);
        let mut peaks = vec![0.0f32; cols];
        let view_frames = (f1 - f0).max(1);
        let right = sd.right.as_deref();
        for i in 0..cols {
            let sa = f0 + i * view_frames / cols;
            let sb = (f0 + (i + 1) * view_frames / cols).max(sa + 1).min(f1);
            let mut m = 0.0f32;
            for j in sa..sb {
                let mut v = sd.left.get(j).copied().unwrap_or(0.0).abs();
                if let Some(r) = right { v = v.max(r.get(j).copied().unwrap_or(0.0).abs()); }
                if v > m { m = v; }
            }
            peaks[i] = m.clamp(0.0, 1.0);
        }
        let p = ui.painter_at(rect);
        crate::waveform::paint(&p, rect, &peaks, None, &crate::waveform::WaveStyle::default());
        if let Some((s0, s1)) = self.groove_sample_sel {
            let lo = s0.min(s1); let hi = s0.max(s1);
            let x0 = rect.left() + ((lo - a) / span).clamp(0.0, 1.0) * rect.width();
            let x1 = rect.left() + ((hi - a) / span).clamp(0.0, 1.0) * rect.width();
            if x1 > x0 {
                p.rect_filled(Rect::from_min_max(pos2(x0, rect.top()), pos2(x1, rect.bottom())), 0.0, Color32::from_rgba_unmultiplied(120, 190, 255, 45));
            }
        }

        if zoom > 1.01 {
            ui.add(egui::Slider::new(&mut self.groove_sample_scroll, 0.0..=(1.0 - span).max(0.001)).show_value(false));
        }
        if !self.groove_sample_msg.is_empty() {
            ui.label(egui::RichText::new(&self.groove_sample_msg).size(10.0).color(T.accent));
        }

        if let Some(op) = op {
            self.groove_sample_msg = match self.apply_groove_sample_op(shared, pad, slot, op, sample_rate as u32) {
                Ok(m) => m,
                Err(e) => format!("⚠ {}", e),
            };
        }
    }

    fn apply_groove_sample_op(&mut self, shared: &Arc<SessionShared>, pad: usize, slot: usize, op: lanzadera_engine::sample_edit::SampleOp, sample_rate: u32) -> Result<String, String> {
        use lanzadera_engine::sample_edit::SampleOp;
        let sd = shared.caja_store.load_sample(pad, slot).ok_or("No hay muestra cargada")?;
        let ch = if sd.right.is_some() { 2usize } else { 1usize };
        let frames = sd.len();
        if frames == 0 { return Err("Muestra vacía".into()); }
        let mut data = Vec::with_capacity(frames * ch);
        for i in 0..frames {
            data.push(sd.left.get(i).copied().unwrap_or(0.0));
            if ch == 2 { data.push(sd.right.as_ref().and_then(|r| r.get(i).copied()).unwrap_or(0.0)); }
        }
        fn range_frames(lo: f32, hi: f32, frames: usize) -> (usize, usize) {
            let a = (lo.clamp(0.0, 1.0) * frames as f32) as usize;
            let b = (hi.clamp(0.0, 1.0) * frames as f32) as usize;
            (a.min(b), b.max(a))
        }
        match op {
            SampleOp::Gain(g) => for v in &mut data { *v *= g; },
            SampleOp::Normalize => {
                let peak = data.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                if peak < 1e-6 { return Err("La muestra es silencio".into()); }
                let g = 0.99 / peak;
                for v in &mut data { *v *= g; }
            }
            SampleOp::Reverse => {
                for f in 0..frames/2 {
                    let a = f*ch; let b = (frames-1-f)*ch;
                    for c in 0..ch { data.swap(a+c, b+c); }
                }
            }
            SampleOp::Silence(lo, hi) => { let (a,b)=range_frames(lo,hi,frames); if b<=a {return Err("Selección vacía".into());} for f in a..b { for c in 0..ch { data[f*ch+c]=0.0; } } }
            SampleOp::FadeIn(lo, hi) => { let (a,b)=range_frames(lo,hi,frames); if b<=a {return Err("Selección vacía".into());} let n=(b-a) as f32; for f in a..b { let g=(f-a) as f32/n; for c in 0..ch { data[f*ch+c]*=g; } } }
            SampleOp::FadeOut(lo, hi) => { let (a,b)=range_frames(lo,hi,frames); if b<=a {return Err("Selección vacía".into());} let n=(b-a) as f32; for f in a..b { let g=1.0-(f-a) as f32/n; for c in 0..ch { data[f*ch+c]*=g; } } }
            SampleOp::Trim(_, _) => return Err("Trim no disponible en editor embebido de Groove".into()),
        }

        let dir = std::path::PathBuf::from("/tmp/lanzadera_groove_edits");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let mut dest = dir.join(format!("pad{}_slot{}_edit.wav", pad, slot));
        let mut n = 1;
        while dest.exists() { dest = dir.join(format!("pad{}_slot{}_edit{}.wav", pad, slot, n)); n += 1; }
        let spec = hound::WavSpec { channels: ch as u16, sample_rate, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
        let mut w = hound::WavWriter::create(&dest, spec).map_err(|e| e.to_string())?;
        for v in &data { w.write_sample(*v).map_err(|e| e.to_string())?; }
        w.finalize().map_err(|e| e.to_string())?;

        let (left, right) = if ch == 2 {
            let mut l = Vec::with_capacity(frames);
            let mut r = Vec::with_capacity(frames);
            for f in 0..frames { l.push(data[f*2]); r.push(data[f*2+1]); }
            (l, Some(r))
        } else {
            (data, None)
        };
        let sample = std::sync::Arc::new(lanzadera_engine::caja_drums::sample::Sample { left, right });
        shared.caja_store.store_sample(pad, slot, sample);
        self.groove_wave_cache[pad] = None;
        Ok(format!("Aplicado en pad {} slot {}", pad + 1, slot + 1))
    }

    fn groove_rack_strip(&mut self, ui: &mut egui::Ui, shared: &Arc<SessionShared>) {
        use caja_core::pieces::{PAD_ORDER, PIECES};
        use caja_core::pattern::STEPS_PER_BAR;
        let (pattern, notes, active, bars) = match &self.groove_snap {
            Some(s) => (s.pattern.clone(), s.notes, s.active, s.pattern.bars.max(1)),
            None => return,
        };
        let total_steps = pattern.steps();
        let per_page = 1usize;
        let pages = bars.div_ceil(per_page).max(1);
        self.groove_bar = self.groove_bar.min(pages.saturating_sub(1));
        let abs_step_now = shared.caja_sched.current_abs_step.load(std::sync::atomic::Ordering::Relaxed);
        let playing = groove_head(shared) != 255;
        if self.groove_follow_playhead && playing && abs_step_now != u32::MAX {
            self.groove_bar = (abs_step_now as usize / STEPS_PER_BAR).min(pages.saturating_sub(1));
        }
        let first = (self.groove_bar * per_page * STEPS_PER_BAR).min(total_steps.saturating_sub(1));
        let cols = STEPS_PER_BAR.min(total_steps.saturating_sub(first));

        self.groove_bar = self.groove_bar.min(pages.saturating_sub(1));
        shared.groove_bar.store(self.groove_bar, std::sync::atomic::Ordering::Relaxed);
        shared.groove_follow_playhead.store(self.groove_follow_playhead, std::sync::atomic::Ordering::Relaxed);
        shared.mk3_follow_playhead.store(self.groove_follow_playhead, std::sync::atomic::Ordering::Relaxed);

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("RACK / PATRÓN").size(11.0).strong().color(T.accent_gold));
            ui.label(egui::RichText::new(format!("Slot {}", active + 1)).size(10.0).color(T.text_dim));
            ui.separator();
            if ui.selectable_label(self.groove_follow_playhead, "Seguir").clicked() {
                self.groove_follow_playhead = true;
            }
            if ui.selectable_label(!self.groove_follow_playhead, "Editar").clicked() {
                self.groove_follow_playhead = false;
            }
            ui.add_enabled_ui(!self.groove_follow_playhead, |ui| {
                if ui.small_button("◀").clicked() { self.groove_bar = self.groove_bar.saturating_sub(1); shared.groove_bar.store(self.groove_bar, std::sync::atomic::Ordering::Relaxed); }
                ui.label(egui::RichText::new(format!("Bar {}/{}", self.groove_bar + 1, pages)).size(10.0));
                if ui.small_button("▶").clicked() { self.groove_bar = (self.groove_bar + 1).min(pages - 1); shared.groove_bar.store(self.groove_bar, std::sync::atomic::Ordering::Relaxed); }
            });
            if ui.small_button("↻").on_hover_text("Ir al compás actual").clicked() {
                let abs_step_now = shared.caja_sched.current_abs_step.load(std::sync::atomic::Ordering::Relaxed);
                if abs_step_now != u32::MAX {
                    self.groove_bar = (abs_step_now as usize / STEPS_PER_BAR).min(pages.saturating_sub(1));
                    shared.groove_bar.store(self.groove_bar, std::sync::atomic::Ordering::Relaxed);
                }
            }
            ui.separator();
            let sel_pad = self.groove_sel_pad.min(PAD_ORDER.len() - 1);
            ui.label(egui::RichText::new(PIECES[PAD_ORDER[sel_pad]].name).size(10.0).color(PAD_PALETTE[sel_pad % PAD_PALETTE.len()]));
            if ui.button("Abrir Step").clicked() { self.groove_tab = GrooveScreenTab::Step; self.groove_main_focus = GrooveMainFocus::Rack; }
        });

        let sel_pad = self.groove_sel_pad.min(PAD_ORDER.len() - 1);
        let row_h = 34.0f32;
        let name_w = 86.0;
        let avail_w = (ui.available_width() - name_w - 12.0).max(80.0);
        let cw = (avail_w / cols.max(1) as f32).floor().max(14.0);
        let (outer, _) = ui.allocate_exact_size(vec2(name_w + cw * cols as f32 + 4.0, row_h + 4.0), Sense::hover());
        let paint = ui.painter_at(outer);
        let x0 = outer.min.x + name_w;
        let y0 = outer.min.y;
        let pc = PAD_ORDER[sel_pad];
        let hue = PAD_PALETTE[sel_pad % PAD_PALETTE.len()];
        paint.text(pos2(outer.min.x + 4.0, y0 + row_h * 0.5), Align2::LEFT_CENTER, trunc(PIECES[pc].name, 12), FontId::proportional(10.0), hue);
        paint.text(pos2(outer.min.x + name_w - 6.0, y0 + row_h * 0.5), Align2::RIGHT_CENTER, format!("{}", notes[pc]), FontId::proportional(9.0), T.text_dim);
        for c in 0..cols {
            let st = first + c;
            if st >= total_steps { break; }
            let cell = pattern.cell[pc][st];
            let slot = Rect::from_min_size(pos2(x0 + c as f32 * cw + 1.0, y0 + 1.0), vec2(cw - 2.0, row_h - 2.0));
            let is_active = playing && abs_step_now != u32::MAX && abs_step_now as usize == st;
            paint.rect_filled(slot, 2.0, if is_active { T.accent.gamma_multiply(0.18) } else { T.panel });
            if cell.on() {
                let h = (slot.height() * f32::from(cell.vel) / 127.0).max(3.0);
                let bar = Rect::from_min_max(pos2(slot.min.x, slot.max.y - h), slot.max);
                if cell.ghost { paint.rect_stroke(bar, 2.0, Stroke::new(1.5, hue)); } else { paint.rect_filled(bar, 2.0, hue); }
            }
            if st % STEPS_PER_BAR == 0 {
                paint.rect_filled(Rect::from_min_size(pos2(x0 + c as f32 * cw - 1.0, y0), vec2(2.0, row_h)), 0.0, T.text_dim.gamma_multiply(0.6));
            }
        }
        if playing && abs_step_now != u32::MAX {
            let abs = abs_step_now as usize;
            if abs >= first && abs < first + cols {
                let hx = x0 + (abs - first) as f32 * cw;
                paint.rect_filled(Rect::from_min_size(pos2(hx, y0), vec2(cw, row_h)), 0.0, T.accent_green.gamma_multiply(0.10));
            }
        }
    }

    #[allow(dead_code)]
    fn groove_mixer_tab(&mut self, ui: &mut egui::Ui, shared: &Arc<SessionShared>) {
        use caja_core::pieces::{PAD_ORDER, PIECES};

        // Misma fuente de verdad que el MK3 y el hilo de audio.
        let kit = shared.kit.clone();
        let cfg = kit.snapshot_cfg();

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("GROOVE MIXER")
                .color(T.accent_gold).size(12.0).strong());
            ui.label(egui::RichText::new("· estilo consola analógica")
                .color(T.text_dim).size(10.0));
        });
        ui.label(egui::RichText::new(
            "MK3: pad selecciona · ENCODER = pan · touchstrip = volumen"
        ).color(T.text_dim).size(10.0));
        ui.add_space(6.0);

        // Strips verticales (mismo lenguaje visual que el mixer principal)
        egui::ScrollArea::horizontal().id_source("gm_mixer_scroll")
            .auto_shrink([false, false]).show(ui, |ui| {
            ui.horizontal_top(|ui| {
                for i in 0..cfg.len() {
                    let pc = PAD_ORDER[i.min(PAD_ORDER.len() - 1)];
                    let sel = self.groove_sel_pad == i;
                    let border = if sel {
                        egui::Stroke::new(1.5, T.accent_gold)
                    } else {
                        egui::Stroke::new(1.0, T.strip_border)
                    };

                    egui::Frame::none()
                        .fill(T.strip_bg)
                        .rounding(Rounding::same(5.0))
                        .stroke(border)
                        .inner_margin(4.0)
                        .show(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.set_width(56.0);

                            // Nombre de la celda (click = seleccionar)
                            let name = trunc(PIECES[pc].name, 7);
                            let col = if sel { T.accent_gold } else { T.text_bright };
                            let head = ui.add(
                                egui::Label::new(egui::RichText::new(format!("{} {}", i + 1, name))
                                    .size(10.0).color(col).strong())
                                    .sense(egui::Sense::click()),
                            );
                            if head.clicked() {
                                self.groove_sel_pad = i;
                                self.inspector_focus = InspectorFocus::GrooveMixer;
                            }

                            // Pan knob
                            let mut pan = cfg[i].pan_norm();
                            if widgets::knob(ui, &mut pan, -1.0, 1.0, 0.0, "Pan") {
                                self.groove_sel_pad = i;
                                kit.set_pan(i, (pan.clamp(-1.0, 1.0) * 1000.0).round() as i16);
                            }

                            // Gain como fader (0..2 → normalizado visual 0..1)
                            let mut gain = cfg[i].gain_linear();
                            let mut gain_norm = (gain / 2.0).clamp(0.0, 1.0);
                            if widgets::fader(ui, &mut gain_norm, 90.0, 0.0) {
                                gain = gain_norm * 2.0;
                                self.groove_sel_pad = i;
                                kit.set_gain(i, (gain.clamp(0.0, 2.0) * 1000.0).round() as u16);
                            }
                            let db = if gain <= 0.0001 { "-inf".into() }
                                     else { format!("{:+.1}", 20.0 * gain.log10()) };
                            ui.label(egui::RichText::new(db).size(9.0).color(T.text_dim));

                            // M / S
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 2.0;
                                let mut mute = cfg[i].mute;
                                if ui.add(egui::Button::new(egui::RichText::new("M").size(10.0))
                                    .fill(if mute { Color32::from_rgb(240, 200,  70).gamma_multiply(0.85) } else { T.panel_raised })
                                    .min_size(vec2(18.0, 16.0))).clicked()
                                {
                                    mute = !mute;
                                    self.groove_sel_pad = i;
                                    kit.set_mute(i, mute);
                                }
                                let mut solo = cfg[i].solo;
                                if ui.add(egui::Button::new(egui::RichText::new("S").size(10.0))
                                    .fill(if solo { Color32::from_rgb(240, 152,  72).gamma_multiply(0.85) } else { T.panel_raised })
                                    .min_size(vec2(18.0, 16.0))).clicked()
                                {
                                    solo = !solo;
                                    self.groove_sel_pad = i;
                                    kit.set_solo(i, solo);
                                }
                            });

                            ui.label(egui::RichText::new(format!("n{}", 36 + i))
                                .size(9.0).color(T.text_dim));
                        });
                    });
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
        let apply_layout = self.window_layout_apply_frames > 0;
        let default_sz = egui::vec2(560.0, 560.0);
        let saved_sz = self.windows.browser_size.filter(|s| s.x > 100.0 && s.y > 100.0);

        let mut win = egui::Window::new("📁 Browser")
            .open(&mut open)
            .constrain(false)
            .max_size([8000.0, 8000.0])
            .min_size([420.0, 360.0])
            .resizable(true)
            .frame(Self::window_frame());
        if apply_layout {
            if let Some(s) = saved_sz { win = win.fixed_size(s); }
            else { win = win.default_size(default_sz); }
            if let Some(p) = self.windows.browser_pos { win = win.current_pos(p); }
        } else {
            win = win.default_size(saved_sz.unwrap_or(default_sz));
            if let Some(p) = self.windows.browser_pos { win = win.default_pos(p); }
        }
        let resp = win.show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut self.browser_tab, BrowserTab::Files, "📂 Directorio");
                ui.selectable_value(&mut self.browser_tab, BrowserTab::Instruments, "🎹 Instrumentos");
                ui.selectable_value(&mut self.browser_tab, BrowserTab::NoteFx, "🎼 Note FX");
                ui.selectable_value(&mut self.browser_tab, BrowserTab::Fx, "🎛 FX");
            });
            ui.separator();

            match self.browser_tab {
                BrowserTab::Files => self.browser_files_tab(ui, shared),
                BrowserTab::Instruments => self.browser_plugins_tab(ui, shared, BrowserTarget::Instrument),
                BrowserTab::NoteFx => self.browser_plugins_tab(ui, shared, BrowserTarget::NoteFx),
                BrowserTab::Fx => self.browser_plugins_tab(ui, shared, BrowserTarget::Fx),
            }
        });

        if resp.is_some() {
            let rect = ctx.memory(|mem| mem.area_rect(egui::Id::new("📁 Browser")));
            if let Some(r) = rect {
                self.windows.browser_pos = Some(r.min);
                if !apply_layout || self.window_layout_apply_frames == 1 {
                    self.windows.browser_size = Some(r.size());
                }
            }
        }
        self.windows.browser_visible = open;
    }

    fn browser_files_tab(&mut self, ui: &mut egui::Ui, shared: &Arc<SessionShared>) {
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
                    .desired_width(180.0)
                    .hint_text("filtrar archivos")
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
                } else if is_midi_file(&path) {
                    let label = format!("🎼 {}", name);
                    let resp = ui.dnd_drag_source(
                        egui::Id::new(("browser_midi", &path)),
                        BrowserFile(path.to_string_lossy().into_owned()),
                        move |ui| ui.button(label),
                    ).inner;
                    if resp.clicked() {
                        let (track, scene) = {
                            let st = shared.state.lock().unwrap();
                            (st.selected_track, st.selected_scene)
                        };
                        if let Ok(mut q) = shared.load_queue.lock() {
                            q.push_back(lanzadera_engine::shared::LoadRequest {
                                track,
                                scene,
                                path: path.to_string_lossy().to_string(),
                            });
                        }
                    }
                } else if is_audio_file(&path) {
                    let label = format!("🎵 {}", name);
                    let resp = ui.dnd_drag_source(
                        egui::Id::new(("browser_audio", &path)),
                        BrowserFile(path.to_string_lossy().into_owned()),
                        move |ui| ui.button(label),
                    ).inner;
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
    }

    fn browser_plugins_tab(&mut self, ui: &mut egui::Ui, shared: &Arc<SessionShared>, target: BrowserTarget) {
        use lanzadera_core::PluginKind;
        self.browser_target = target;
        let want = match target {
            BrowserTarget::Instrument => PluginKind::Instrument,
            BrowserTarget::NoteFx => PluginKind::NoteFx,
            BrowserTarget::Fx => PluginKind::Fx,
        };
        let t = { shared.state.lock().unwrap().selected_track };

        if self.browser_list.is_none() && self.scan_rx.is_none() {
            self.browser_rescan();
        }

        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(format!("Destino: Pista {}", t + 1)).color(T.text_dim));
            if lanzadera_engine::CLAP_ENABLED
                && ui.selectable_label(self.browser_mode == BrowserMode::Clap, "CLAP").clicked() {
                self.browser_mode = BrowserMode::Clap;
                self.browser_list = None;
            }
            if lanzadera_engine::VST3_ENABLED
                && ui.selectable_label(self.browser_mode == BrowserMode::Vst3, "VST3").clicked() {
                self.browser_mode = BrowserMode::Vst3;
                self.browser_list = None;
            }
            ui.checkbox(&mut self.browser_show_all, "Mostrar todos");
            if ui.button("⟳").on_hover_text("Re-escanear plugins").clicked() {
                self.browser_list = None;
            }
        });
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.browser_plugin_filter)
                .desired_width(220.0)
                .hint_text("buscar plugin por nombre o ruta"));
            if ui.small_button("✕").clicked() { self.browser_plugin_filter.clear(); }
        });
        ui.separator();

        let mut chosen: Option<(String, String)> = None;
        if self.scan_rx.is_some() && self.browser_list.is_none() {
            ui.label("⏳ Escaneando plugins…");
            if let Some(c) = &self.ctx_cache { c.request_repaint_after(std::time::Duration::from_millis(120)); }
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let Some(list) = &self.browser_list else { return; };
            if list.is_empty() {
                ui.label(match self.browser_mode {
                    BrowserMode::Clap => "Sin plugins CLAP encontrados.",
                    BrowserMode::Vst3 => "Sin plugins VST3 encontrados.",
                });
                return;
            }
            let filter = self.browser_plugin_filter.to_lowercase();
            let mut shown = 0usize;
            for (path, id, name) in list {
                let kind = self.browser_kinds.get(&(path.clone(), id.clone())).copied().unwrap_or(PluginKind::Unknown);
                if !(self.browser_show_all || kind == want || kind == PluginKind::Unknown) { continue; }
                if !filter.is_empty() {
                    let hay = format!("{} {}", name, path).to_lowercase();
                    if !hay.contains(&filter) { continue; }
                }
                shown += 1;
                ui.horizontal(|ui| {
                    if ui.button("➕").clicked() { chosen = Some((path.clone(), id.clone())); }
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(name).strong());
                            ui.label(egui::RichText::new(kind.label()).size(9.0).color(T.accent_gold));
                        });
                        ui.label(egui::RichText::new(trunc(path, 56)).size(9.0).color(T.text_dim));
                    });
                });
            }
            if shown == 0 {
                ui.label(egui::RichText::new("Sin resultados para el filtro/tipo actual.").color(T.text_dim));
            }
        });

        if let Some((path, id)) = chosen {
            match target {
                BrowserTarget::Instrument => {
                    shared.set_plugin(t, Some(path), Some(id));
                    self.msg = "Cargando plugin (reiniciando audio)…".into();
                }
                BrowserTarget::NoteFx => {
                    let has_instr = shared.state.lock().unwrap().tracks.get(t)
                        .map(|tr| tr.plugin_path.is_some()).unwrap_or(false);
                    shared.set_note_fx(t, Some(path), Some(id));
                    self.msg = if has_instr {
                        format!("Cargando Note FX en Pista {} (reiniciando audio)…", t + 1)
                    } else {
                        format!("Pista {} sin instrumento: Note FX se aplicará al cargar uno", t + 1)
                    };
                }
                BrowserTarget::Fx => {
                    shared.set_fx(t, Some(path), Some(id));
                    self.msg = format!("Cargando FX en Pista {} (reiniciando audio)…", t + 1);
                }
            }
            self.browser_list = None;
        }
    }

    fn global_inspector_window(&mut self, ctx: &egui::Context, shared: &Arc<SessionShared>) {
        let mut open = self.windows.inspector_visible;
        let apply_layout = self.window_layout_apply_frames > 0;
        let default_sz = egui::vec2(300.0, 560.0);
        let saved_sz = self.windows.inspector_size.filter(|s| s.x > 100.0 && s.y > 100.0);

        let mut win = egui::Window::new("🔍 Inspector")
            .open(&mut open)
            .constrain(false)
            .max_size([8000.0, 8000.0])
            .min_size([240.0, 320.0])
            .resizable(true)
            .frame(Self::window_frame());
        if apply_layout {
            if let Some(s) = saved_sz { win = win.fixed_size(s); }
            else { win = win.default_size(default_sz); }
            if let Some(p) = self.windows.inspector_pos { win = win.current_pos(p); }
        } else {
            win = win.default_size(saved_sz.unwrap_or(default_sz));
            if let Some(p) = self.windows.inspector_pos { win = win.default_pos(p); }
        }
        let resp = win.show(ctx, |ui| {
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
                if groove_focused {
                    if self.groove_chords || shared.groove_sub_mode.load(std::sync::atomic::Ordering::Relaxed) == 3 {
                        self.inspector_groove_chords(ui, shared);
                        ui.separator();
                        self.inspector_groove_cell(ui, shared);
                    } else {
                        match self.groove_tab {
                            GrooveScreenTab::Main => {
                                match self.groove_detail_tab {
                                    GrooveDetailTab::Chord => self.inspector_groove_chords(ui, shared),
                                    GrooveDetailTab::Params => self.inspector_groove_cell(ui, shared),
                                }
                            }
                            GrooveScreenTab::Step => {
                                if self.groove_view == 1 || self.groove_event {
                                    self.inspector_groove_cell(ui, shared);
                                } else {
                                    self.inspector_groove_step(ui, shared);
                                }
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

        if resp.is_some() {
            let rect = ctx.memory(|mem| mem.area_rect(egui::Id::new("🔍 Inspector")));
            if let Some(r) = rect {
                self.windows.inspector_pos = Some(r.min);
                if !apply_layout || self.window_layout_apply_frames == 1 {
                    self.windows.inspector_size = Some(r.size());
                }
            }
        }
        self.windows.inspector_visible = open;
    }

    fn mixer_window(&mut self, ctx: &egui::Context, shared: &Arc<SessionShared>, audio_on: bool) {
        let mut open = self.windows.mixer_visible;
        let apply_layout = self.window_layout_apply_frames > 0;
        let default_sz = egui::vec2(900.0, 260.0);
        let saved_sz = self.windows.mixer_size.filter(|s| s.x > 100.0 && s.y > 100.0);

        let mut win = egui::Window::new("🎚 Mixer")
            .open(&mut open)
            .constrain(false)
            .max_size([8000.0, 8000.0])
            .min_size([420.0, 160.0])
            .resizable(true)
            .frame(Self::window_frame());
        if apply_layout {
            if let Some(s) = saved_sz { win = win.fixed_size(s); }
            else { win = win.default_size(default_sz); }
            if let Some(p) = self.windows.mixer_pos { win = win.current_pos(p); }
        } else {
            win = win.default_size(saved_sz.unwrap_or(default_sz));
            if let Some(p) = self.windows.mixer_pos { win = win.default_pos(p); }
        }
        let resp = win.show(ctx, |ui| {
            self.mixer(ui, shared, audio_on);
        });
        if resp.is_some() {
            let rect = ctx.memory(|mem| mem.area_rect(egui::Id::new("🎚 Mixer")));
            if let Some(r) = rect {
                self.windows.mixer_pos = Some(r.min);
                if !apply_layout || self.window_layout_apply_frames == 1 {
                    self.windows.mixer_size = Some(r.size());
                }
            }
        }

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

fn is_midi_file(path: &std::path::Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => matches!(ext.to_ascii_lowercase().as_str(), "mid" | "midi"),
        None => false,
    }
}

/// Payload del arrastre browser -> celda (Fase G3).
///
/// Newtype sobre String a proposito: si el payload fuera un String pelado,
/// cualquier otro arrastre de texto de la app podria colisionar con este.
/// egui exige `Any + Send + Sync`; String lo cumple.
#[derive(Clone)]
#[allow(dead_code)]
pub struct BrowserFile(pub String);

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
    #[allow(dead_code)]
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
        let (cell, loaded) = {
            let lib = library_state::shared();
            let g = lib.lock().unwrap_or_else(|e| e.into_inner());
            let cell = g.lib.cells.get(pad).cloned().unwrap_or_default();
            let loaded = g.loaded.get(pad).copied().unwrap_or(0);
            (cell, loaded)
        };
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
            if let Some(rel) = cell.get(slot).cloned() {
                // Aplicar inmediatamente en RT/store; persistencia en background para no congelar UI.
                kit.set_pin(pad, Some(rel.clone()));
                kit.set_lock(pad, caja_core::kit::P_SAMPLE, true);
                _shared.caja_store.pin_idx[pad].store(slot as i32, std::sync::atomic::Ordering::Relaxed);
                _shared.caja_store.pin_missing[pad].store(false, std::sync::atomic::Ordering::Relaxed);
                let rel_bg = rel.clone();
                std::thread::spawn(move || {
                    let lib = lanzadera_engine::caja_drums::library_state::shared();
                    let mut g = lib.lock().unwrap_or_else(|e| e.into_inner());
                    g.set_pin(pad, Some((slot, rel_bg)));
                    g.save();
                });
                self.msg = format!("Muestra fijada: {}", rel);
            } else {
                self.msg = "No hay muestra actual para fijar".to_string();
            }
        }
        if c.pin.is_some() && ui.button("Desbloquear").clicked() {
            kit.set_pin(pad, None);
            kit.set_lock(pad, caja_core::kit::P_SAMPLE, false);
            _shared.caja_store.pin_idx[pad].store(-1, std::sync::atomic::Ordering::Relaxed);
            _shared.caja_store.pin_missing[pad].store(false, std::sync::atomic::Ordering::Relaxed);
            std::thread::spawn(move || {
                let lib = lanzadera_engine::caja_drums::library_state::shared();
                let mut g = lib.lock().unwrap_or_else(|e| e.into_inner());
                g.set_pin(pad, None);
                g.save();
            });
            self.msg = "Pin quitado".to_string();
        }

        ui.separator();
        // ── Cambio de muestra por celda ──
        ui.label(egui::RichText::new("Cambio de muestra").color(T.accent).size(12.0));
        ui.horizontal_wrapped(|ui| {
            for (bars, lbl) in [(0u8, "Al toque"), (1, "1c"), (2, "2c"), (3, "3c"), (4, "4c")] {
                let on = c.rate == bars;
                if ui.add(egui::Button::new(lbl).fill(if on { T.accent } else { T.panel }))
                    .on_hover_text(if bars == 0 { "Cambia en cada golpe".to_string() } else { format!("Cada {} compases", bars) })
                    .clicked()
                {
                    kit.set_rate(pad, bars);
                }
            }
        });

        ui.separator();
        // ── Síntesis por celda ──
        ui.label(egui::RichText::new("Síntesis por celda").color(T.accent).size(12.0));
        use caja_core::kit::{ParamCfg, P_DECAY, P_FILTER, P_PITCH, P_SAMPLE};
        use lanzadera_engine::caja_drums::engine::{cutoff_hz, decay_seconds, pitch_semitones};
        let mut param = c.param;
        let mut changes: Vec<(usize, ParamCfg)> = Vec::new();
        for (idx, lbl) in [(P_SAMPLE, "Muestra"), (P_PITCH, "Pitch"), (P_DECAY, "Decay"), (P_FILTER, "Filtro")] {
            let center = param[idx].base as f32 / 1000.0;
            let readout = match idx {
                P_SAMPLE => format!("{}% banco", (param[idx].range / 10).max(1)),
                P_PITCH => format!("{:+.1} st", pitch_semitones(center)),
                P_DECAY => match decay_seconds(center) { Some(s) => format!("{:.0} ms", s * 1000.0), None => "sin recorte".to_string() },
                _ => match cutoff_hz(center) { Some(h) => format!("{:.0} Hz", h), None => "abierto".to_string() },
            };
            ui.label(egui::RichText::new(format!("{} · {}", lbl, readout)).color(T.text_dim).size(11.0));
            let mut base = i32::from(param[idx].base);
            let mut range = i32::from(param[idx].range);
            let mut prob = i32::from(param[idx].prob);
            let mut dirty = false;
            ui.horizontal(|ui| {
                dirty |= ui.add(egui::Slider::new(&mut base, 0..=1000).text("base")).changed();
                dirty |= ui.add(egui::Slider::new(&mut range, 0..=1000).text("rango")).changed();
            });
            dirty |= ui.add(egui::Slider::new(&mut prob, 0..=1000).text("prob ‰")).changed();
            if dirty {
                param[idx] = ParamCfg { base: base.clamp(0, 1000) as u16, range: range.clamp(0, 1000) as u16, prob: prob.clamp(0, 1000) as u16 };
                changes.push((idx, param[idx]));
            }
        }
        for (idx, pc) in changes { kit.set_param(pad, idx, pc); }
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

        ui.heading("SAMPLES · Biblioteca");
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
        });
        ui.add_space(4.0);
        ui.label(egui::RichText::new("Cambio de muestra de la celda").color(T.accent).size(12.0));
        ui.label(egui::RichText::new("Cada cuánto re-tira el dado de muestra (por defecto 4c)").size(10.0).color(T.text_dim));
        ui.horizontal_wrapped(|ui| {
            for (bars, lbl) in [(0u8, "Al toque"), (1, "1c"), (2, "2c"), (3, "3c"), (4, "4c")] {
                let on = c.rate == bars;
                let tip = if bars == 0 {
                    "Cambia la muestra en cada golpe (dado activo)".to_string()
                } else {
                    format!("Re-tirar el dado cada {} compases", bars)
                };
                if ui.add(egui::Button::new(lbl).fill(if on { T.accent } else { T.panel }))
                    .on_hover_text(tip)
                    .clicked()
                {
                    shared.kit.set_rate(pad, bars);
                    self.msg = if bars == 0 {
                        "Muestra: al toque".to_string()
                    } else {
                        format!("Muestra: cambia cada {} compases", bars)
                    };
                }
            }
        });
        if ui.button("💥 Todos golpe").clicked() {
            for p in 0..lanzadera_engine::groove_kit::NUM_CELLS as u8 {
                shared.apply_control(ControlCmd::LivePadOn { pad: p, vel: 100 });
            }
        }

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
