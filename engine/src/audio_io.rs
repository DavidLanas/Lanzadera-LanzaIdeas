// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! Audio I/O con cpal (Fase 3B). Render por bloques:
//!   1) frame a frame: ClipEngine (sample-accurate) + notas MIDI + archivo + sinte
//!   2) por pista con plugin CLAP: process() del bloque con eventos time-stamped
//!   3) frame a frame: EQ → pan/volumen → master
//! Sin locks, sin alloc ni I/O en el callback: todos los buffers se reservan
//! antes de arrancar el stream.
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use lanzadera_core::control::ControlCmd;
use lanzadera_core::{ClipState, SessionAudio, MAX_TRACKS, TICKS_PER_BAR};
use ringbuf::traits::Consumer;
use std::sync::Mutex;

type RecSlot = Option<(usize, usize, crate::audio_record::RecordedClip)>;

/// Slot de grabación completada: el RT deposita al parar, el loader drena.
/// Handoff inspirado en Maolan (BSD-2) y MAGDA (GPL-3): el I/O de disco
/// nunca corre en el callback de audio.
pub static REC_SLOT: Mutex<RecSlot> = Mutex::new(None);

/// Pistas cuyo clip activo debe morir ya (borrado desde la UI).
/// La UI hace push; el RT drena con try_lock en array fijo, sin alloc.
/// Mismo handoff que REC_SLOT, inspirado en Maolan (BSD-2).
pub static KILL_TRACKS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// Flag de stop global (⏹ Todo). La UI lo pone; el RT lo consume al
/// detectar la transición playing→stop. Pausa (⏸) NO lo pone, así que
/// el reloj no se resetea y al volver a Play retoma donde quedó.
pub static STOP_ALL_REQ: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

use crate::clip_engine::{ClipEngine, ClipEvent};
use crate::eq::{EqBank, EqParams};
use crate::midi::{MidiMsg, MidiOut, MidiVoice};
use crate::rec::RecNote;
use crate::shared::{SampleData, SessionShared};
use crate::synth::Synth;
use crate::caja_drums::{CajaEngine, CajaLoader, CajaScheduler, CajaStore, default_kit_state};
use caja_core::kit::{PadCfg, PadLive, ALL_LOCK, NUM_PADS};

#[derive(Clone, Copy, Default)]
struct GmBiquad {
    b0: f32, b1: f32, b2: f32, a1: f32, a2: f32,
    z1: f32, z2: f32,
}
impl GmBiquad {
    #[inline] fn reset(&mut self) { self.z1 = 0.0; self.z2 = 0.0; }
    #[inline] fn set_lpf(&mut self, sr: f32, freq: f32, q: f32) {
        let f = freq.clamp(20.0, sr * 0.45);
        let w0 = 2.0 * std::f32::consts::PI * f / sr.max(1.0);
        let c = w0.cos(); let s = w0.sin();
        let alpha = s / (2.0 * q.max(0.1));
        let b0 = (1.0 - c) * 0.5; let b1 = 1.0 - c; let b2 = (1.0 - c) * 0.5;
        let a0 = 1.0 + alpha; let a1 = -2.0 * c; let a2 = 1.0 - alpha;
        self.b0 = b0 / a0; self.b1 = b1 / a0; self.b2 = b2 / a0;
        self.a1 = a1 / a0; self.a2 = a2 / a0;
    }
    #[inline] fn set_hpf(&mut self, sr: f32, freq: f32, q: f32) {
        let f = freq.clamp(20.0, sr * 0.45);
        let w0 = 2.0 * std::f32::consts::PI * f / sr.max(1.0);
        let c = w0.cos(); let s = w0.sin();
        let alpha = s / (2.0 * q.max(0.1));
        let b0 = (1.0 + c) * 0.5; let b1 = -(1.0 + c); let b2 = (1.0 + c) * 0.5;
        let a0 = 1.0 + alpha; let a1 = -2.0 * c; let a2 = 1.0 - alpha;
        self.b0 = b0 / a0; self.b1 = b1 / a0; self.b2 = b2 / a0;
        self.a1 = a1 / a0; self.a2 = a2 / a0;
    }
    #[inline] fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}


/// Tamaño máximo de bloque que procesamos de una vez (los callbacks mayores
/// se trocean). Los plugins se activan con este max_frames_count.
pub const MAX_BLOCK: usize = 4096;
#[cfg(feature = "clap")]
const MAX_EVENTS: usize = crate::plugin_events::MAX_PLUGIN_EVENTS;

#[cfg(feature = "clap")]
use crate::clap_host::{self, Processor};
#[cfg(feature = "vst3")]
use crate::vst3_slot;
#[cfg(feature = "clap")]
use clack_host::prelude::*;
#[cfg(feature = "clap")]
use clack_host::events::io::EventBuffer;

pub struct AudioHandle {
    /// Mantiene vivo el stream; con la feature clap el Drop lo suelta
    /// explícitamente para devolver los procesadores al hilo principal.
    #[allow(dead_code)]
    stream: Option<cpal::Stream>,
    pub sample_rate: u32,
    #[allow(dead_code)] // mantiene vivo el hilo del loader
    caja_loader: Option<CajaLoader>,
    #[cfg(feature = "clap")]
    /// (indice de pista, instancia). El indice permite abrir la GUI de la
    /// pista correcta: el orden de carga no es el orden de pista.
    instances: Vec<(usize, clap_host::Instance)>,
    /// Instancias de Note FX y FX CLAP. Id pseudo-pista (MAX_TRACKS + t para Note FX,
    /// 2*MAX_TRACKS + t para FX) para no chocar con las tablas por pista del instrumento.
    #[cfg(feature = "clap")]
    fx_instances: Vec<(usize, clap_host::Instance)>,
    #[cfg(feature = "clap")]
    retire_rx: Option<std::sync::mpsc::Receiver<clap_host::Stopped>>,
    /// Ventanas X11 de las GUI CLAP embebidas (plugins sin GUI flotante).
    #[cfg(feature = "clap")]
    clap_embed: crate::clap_gui::EmbedHost,
    /// Plugins VST3 vivos (pista, plugin compartido con el hilo de audio).
    /// Esta copia del Arc se suelta la ULTIMA (Drop), en el hilo principal.
    #[cfg(feature = "vst3")]
    vst3_handles: Vec<(usize, std::sync::Arc<std::sync::Mutex<vst3_host::plugin::Plugin>>)>,
    #[cfg(feature = "vst3")]
    vst3_gui: crate::vst3_slot::Vst3Gui,
    /// FX VST3 vivos (se sueltan en el hilo de teardown junto a los demás).
    #[cfg(feature = "vst3")]
    fx_vst3_handles: Vec<(usize, std::sync::Arc<std::sync::Mutex<vst3_host::plugin::Plugin>>)>,
    /// (pista, ranura 0=instrumento 1=Note FX 2=FX, ruta) de cada plugin cargado: sirve para
    /// saber a qué plugin pertenece el estado que se captura.
    loaded: Vec<(usize, u8, usize, String)>,
    #[allow(dead_code)] // mantiene vivo el stream de entrada
    in_stream: Option<crate::audio_input::AudioInputStream>,
}

impl AudioHandle {
    /// Pide a cada plugin su estado y lo guarda (base64) en la sesión viva, para que el
    /// proyecto lo incluya y sobreviva a un reinicio del audio. Hilo principal.
    /// Solo se guarda el estado de plugins cuya ruta sigue siendo la de la pista.
    pub fn capture_plugin_states(&mut self, shared: &SessionShared) {
        // (pista, tipo 0=instrumento 1=Note FX 2=FX, índice de slot, estado en base64)
        #[allow(unused_mut)] // sin las features clap/vst3 no se añade nada
        let mut got: Vec<(usize, u8, usize, String)> = Vec::new();
        #[cfg(feature = "clap")]
        {
            for (t, inst) in self.instances.iter_mut() {
                if let Some(b) = clap_host::save_state(inst) {
                    got.push((*t, 0, 0, lanzadera_core::b64::encode(&b)));
                }
            }
            for (id, inst) in self.fx_instances.iter_mut() {
                if let Some((t, kind, i)) = crate::clap_gui::gui_id_slot(*id) {
                    if let Some(b) = clap_host::save_state(inst) {
                        got.push((t, kind, i, lanzadera_core::b64::encode(&b)));
                    }
                }
            }
        }
        #[cfg(feature = "vst3")]
        {
            for (t, h) in self.vst3_handles.iter() {
                if let Ok(pl) = h.lock() {
                    if let Ok(b) = pl.save_state() { got.push((*t, 0, 0, lanzadera_core::b64::encode(&b))); }
                }
            }
            for (id, h) in self.fx_vst3_handles.iter() {
                if let Some((t, kind, i)) = crate::clap_gui::gui_id_slot(*id) {
                    if let Ok(pl) = h.lock() {
                        if let Ok(b) = pl.save_state() { got.push((t, kind, i, lanzadera_core::b64::encode(&b))); }
                    }
                }
            }
        }
        if got.is_empty() { return; }
        let loaded = self.loaded.clone();
        shared.edit(|st| {
            for (t, kind, i, b) in got {
                let Some(tr) = st.tracks.get_mut(t) else { continue };
                let cur = match kind {
                    0 => tr.plugin_path.clone(),
                    1 => tr.note_fx_chain.get(i).map(|sl| sl.path.clone()),
                    _ => tr.fx_chain.get(i).map(|sl| sl.path.clone()),
                };
                if !loaded.iter().any(|(lt, lk, li, lp)| *lt == t && *lk == kind && *li == i && Some(lp) == cur.as_ref()) { continue; }
                match kind {
                    0 => tr.plugin_state = Some(b),
                    1 => { if let Some(sl) = tr.note_fx_chain.get_mut(i) { sl.state = Some(b); } }
                    _ => { if let Some(sl) = tr.fx_chain.get_mut(i) { sl.state = Some(b); } }
                }
            }
        });
    }
}

#[cfg(feature = "clap")]
impl AudioHandle {
    /// Atiende la peticion de abrir/cerrar la interfaz de un plugin (CLAP o
    /// VST3) y mantiene vivas las GUI abiertas. Se llama CADA frame desde el
    /// hilo principal (App::update), nunca desde RT.
    pub fn poll_gui_requests(&mut self) {
        // Ventanas flotantes que el usuario cerro desde su propio boton: hay
        // que acusar recibo con destroy mientras la instancia sigue viva.
        for (t, destroyed) in crate::clap_gui::take_closed() {
            if let Some((_, inst)) = self.instances.iter_mut().find(|(i, _)| *i == t) {
                crate::clap_gui::ack_closed(inst, t, destroyed);
            } else if let Some((_, inst)) = self.fx_instances.iter_mut().find(|(i, _)| *i == t) {
                crate::clap_gui::ack_closed(inst, t, destroyed);
            }
        }
        // GUI CLAP embebidas: cierre por la X y resizes del plugin.
        crate::clap_services::poll(&mut self.instances);
        crate::clap_services::poll(&mut self.fx_instances);
        crate::clap_gui::poll_embedded(&mut self.instances, &mut self.clap_embed);
        crate::clap_gui::poll_embedded(&mut self.fx_instances, &mut self.clap_embed);

        // GUI VST3: run loop del plugin + cierre por la X.
        #[cfg(feature = "vst3")]
        for t in self.vst3_gui.poll(&self.vst3_handles) {
            crate::clap_gui::set_open(t, false);
            println!("[gui] pista {}: el usuario cerro la ventana VST3", t + 1);
        }
        #[cfg(feature = "vst3")]
        for id in self.vst3_gui.poll(&self.fx_vst3_handles) {
            crate::clap_gui::set_open(id, false);
            println!("[gui] FX pista {}: el usuario cerro la ventana VST3", crate::clap_gui::gui_id_track(id) + 1);
        }

        let Some(t) = crate::clap_gui::take_request() else { return };
        let title = format!("Lanzadera · pista {}", t + 1);

        #[cfg(feature = "vst3")]
        if let Some((_, plugin)) = self.vst3_handles.iter().find(|(i, _)| *i == t) {
            match self.vst3_gui.toggle(t, plugin) {
                Ok(open) => {
                    crate::clap_gui::set_open(t, open);
                    println!("[gui] pista {}: {}", t + 1, if open { "abierta (VST3)" } else { "cerrada (VST3)" });
                }
                Err(e) => crate::clap_gui::notify(format!("[gui] pista {}: {}", t + 1, e)),
            }
            return;
        }
        // ── VST3 FX (pseudo-id del hueco) ──
        #[cfg(feature = "vst3")]
        if let Some((_, plugin)) = self.fx_vst3_handles.iter().find(|(i, _)| *i == t) {
            let pista = crate::clap_gui::gui_id_track(t);
            match self.vst3_gui.toggle(t, plugin) {
                Ok(open) => {
                    crate::clap_gui::set_open(t, open);
                    println!("[gui] FX pista {}: {}", pista + 1, if open { "abierta (VST3)" } else { "cerrada (VST3)" });
                }
                Err(e) => crate::clap_gui::notify(format!("[gui] FX pista {}: {}", pista + 1, e)),
            }
            return;
        }

        if let Some((_, inst)) = self.instances.iter_mut().find(|(i, _)| *i == t) {
            match crate::clap_gui::toggle(inst, t, &title, &mut self.clap_embed) {
                Ok(()) => {
                    let (_, open) = crate::clap_gui::state(t);
                    println!("[gui] pista {}: {}", t + 1, if open { "abierta" } else { "cerrada" });
                }
                Err(e) => crate::clap_gui::notify(format!("[gui] pista {}: {}", t + 1, e)),
            }
            return;
        }
        if let Some((_, inst)) = self.fx_instances.iter_mut().find(|(i, _)| *i == t) {
            let pista = crate::clap_gui::gui_id_track(t);
            let kind = if t >= crate::clap_gui::fx_gui_id(0, 0) { "FX" } else { "Note FX" };
            let ftitle = format!("Lanzadera · {} pista {}", kind, pista + 1);
            match crate::clap_gui::toggle(inst, t, &ftitle, &mut self.clap_embed) {
                Ok(()) => {
                    let (_, open) = crate::clap_gui::state(t);
                    println!("[gui] {} pista {}: {}", kind, pista + 1, if open { "abierta" } else { "cerrada" });
                }
                Err(e) => crate::clap_gui::notify(format!("[gui] {} pista {}: {}", kind, pista + 1, e)),
            }
            return;
        }
        crate::clap_gui::notify(format!("[gui] pista {} no tiene plugin cargado", crate::clap_gui::gui_id_track(t) + 1));
    }
}

#[cfg(feature = "clap")]
impl Drop for AudioHandle {
    fn drop(&mut self) {
        // 0) Cerrar las GUI abiertas ANTES de soltar nada: la ventana del
        //    plugin debe destruirse mientras la instancia sigue viva.
        crate::clap_gui::close_all(&mut self.instances, &mut self.clap_embed);
        crate::clap_gui::close_all(&mut self.fx_instances, &mut self.clap_embed);
        for t in 0..crate::clap_gui::MAX_PLUGIN_SLOTS { crate::clap_services::clear_track(t); }
        #[cfg(feature = "vst3")]
        {
            for (t, _) in self.vst3_handles.iter() { crate::clap_gui::set_open(*t, false); }
            for (id, _) in self.fx_vst3_handles.iter() { crate::clap_gui::set_open(*id, false); }
        }
        // 1) Soltar el stream dropea el Renderer, que devuelve los procesadores.
        self.stream.take();
        // 1b) Teardown VST3 en HILO APARTE: cerrar un editor o destruir un
        //     plugin yabridge/wine puede bloquearse (teardown del bridge).
        //     Hacerlo en el hilo principal congelaba la app al cambiar de
        //     plugin. En un hilo suelto, un bloqueo se queda ahí: la app
        //     sigue viva con el nuevo handle de audio.
        #[cfg(feature = "vst3")]
        {
            let mut gui = std::mem::take(&mut self.vst3_gui);
            let handles = std::mem::take(&mut self.vst3_handles);
            let fx_handles = std::mem::take(&mut self.fx_vst3_handles);
            std::thread::Builder::new()
                .name("vst3-teardown".into())
                .spawn(move || {
                    gui.close_all();  // editores primero (la vista antes que el plugin)
                    drop(handles);    // luego los plugins (teardown wine)
                    drop(fx_handles); // y los FX
                })
                .ok();
        }
        // 2) Desactivar cada procesador con su instancia (hilo principal).
        if let Some(rx) = &self.retire_rx {
            while let Ok(p) = rx.recv_timeout(std::time::Duration::from_millis(300)) {
                if let Some((_, inst)) = self.instances.iter_mut().find(|(_, i)| p.matches(i)) {
                    inst.deactivate(p);
                } else if let Some((_, inst)) = self.fx_instances.iter_mut().find(|(_, i)| p.matches(i)) {
                    inst.deactivate(p);
                }
            }
        }
    }
}

struct TrackVoice { audio: Option<Arc<SampleData>>, midi: Option<MidiVoice> }

struct Renderer {
    engine: ClipEngine,
    synth: Synth,
    eq: EqBank,
    voices: [TrackVoice; MAX_TRACKS],
    events: Vec<ClipEvent>,
    midi_out: MidiOut,
    gains: [(f32, f32, bool); MAX_TRACKS],
    peaks: [f32; MAX_TRACKS + 1],
    groove_peak: f32,
    was_playing: bool,
    /// Notas en vivo pendientes de cerrar (grabación): (nota, vel, tick).
    rec_pending: [Option<(u8, u8, u32)>; 32],
    /// Pedal de sustain por pista (solo sinte interno; los plugins reciben el CC 64).
    sustain_down: [bool; MAX_TRACKS],
    /// Notas con note-off retenido por el pedal, por pista. Arrays fijos: sin alloc en RT.
    sustained: [[bool; 128]; MAX_TRACKS],
    rec_on: bool,
    /// (pista, escena) del clip que se está grabando, fijado al empezar la grabación.
    rec_clip: Option<(usize, usize)>,
    /// Último `rec_seq` atendido: cambia cuando se pide una grabación nueva sin parar la anterior.
    rec_seq_seen: u32,
    /// (pista, escena) cuya toma de AUDIO está en curso (se fija al empezar; el cursor puede moverse).
    rec_audio_slot: Option<(usize, usize)>,
    /// Origen del loop de grabación (global_ticks al abrir la puerta).
    /// Las notas se graban relativas a este origen para que el primer
    /// ataque caiga en tick 0 del clip, no en el compás 2 del transporte.
    rec_origin_ticks: u64,
    /// Último global_ticks visto: si retrocede (escena relanzada en play), Groove se reinicia.
    last_global_ticks: u64,
    channels: usize,
    sample_rate: u32,
    bufs: Box<[[[f32; MAX_BLOCK]; 2]; MAX_TRACKS]>,
    #[cfg(feature = "clap")]
    plugins: [Option<Processor>; MAX_TRACKS],
    #[cfg(feature = "vst3")]
    vst3_plugins: [Option<vst3_slot::Vst3Slot>; MAX_TRACKS],
    /// Cadena de Note FX CLAP por pista: transforman las notas, en orden, antes del instrumento.
    #[cfg(feature = "clap")]
    note_fx: [Vec<Processor>; MAX_TRACKS],
    /// Cadena de FX de audio por pista (CLAP o VST3 mezclados), procesada en orden de inserción.
    fx_chains: [Vec<FxInst>; MAX_TRACKS],
    // Scratch fijo del procesado de Note FX / FX: se reserva una vez, sin alloc en RT.
    #[cfg(feature = "clap")]
    nfx_in: EventBuffer,
    #[cfg(feature = "clap")]
    nfx_out: EventBuffer,
    #[cfg(feature = "clap")]
    nfx_src: Box<crate::plugin_events::PluginEvents>,
    #[cfg(feature = "clap")]
    nfx_keep: Box<crate::plugin_events::PluginEvents>,
    #[cfg(feature = "clap")]
    fxin: Box<[[f32; MAX_BLOCK]; 2]>,
    #[cfg(feature = "clap")]
    fx_ports: AudioPorts,
    #[cfg(feature = "clap")]
    ev: [EventBuffer; MAX_TRACKS],
    /// Cola neutral donde escriben los sitios de eventos. Solo flush_clap()
    /// la traduce al formato del plugin, justo antes de process().
    #[cfg(feature = "clap")]
    pev: Box<[crate::plugin_events::PluginEvents; MAX_TRACKS]>,
    #[cfg(feature = "clap")]
    pbuf: Box<[[f32; MAX_BLOCK]; 2]>,
    #[cfg(feature = "clap")]
    out_ports: AudioPorts,
    #[cfg(feature = "clap")]
    steady: u64,
    #[cfg(feature = "clap")]
    retire: Option<std::sync::mpsc::Sender<clap_host::Stopped>>,
    // ── Motor de drums de Caja (Fase 4A) ──
    caja_engine: CajaEngine,
    caja_store: std::sync::Arc<CajaStore>,
    caja_kit: Vec<PadCfg>,
    /// Snapshot del GrooveKit (gain/pan/mute/solo por pad), actualizado por bloque.
    caja_kit_live: [crate::groove_kit::PadMix; NUM_PADS],
    caja_host: [(u32, u8, u8); NUM_PADS],
    caja_buf: Box<[[f32; MAX_BLOCK]; 2]>,
    caja_trigs: [(u32, u8, f32); 64],
    caja_trig_count: usize,
    caja_scheduler: CajaScheduler,
    // FX globales del bus Groove
    groove_fx_filter_l: GmBiquad,
    groove_fx_filter_r: GmBiquad,
    groove_fx_delay_l: Box<[f32; 131072]>,
    groove_fx_delay_r: Box<[f32; 131072]>,
    groove_fx_delay_w: usize,
    groove_fx_tape_env: f32,
    record_engine: crate::audio_record::RecordEngine,
    was_recording: bool,
    audio_in_cons: Option<ringbuf::HeapCons<f32>>,
    audio_in_ch: u16,
    audio_in_scratch: Vec<f32>,
    rec_mono: Vec<f32>,
    /// Envelope residual del click del metrónomo (decay por sample).
    metro_env: f32,
    metro_freq: f32,
    metro_phase: f32,
}

/// Un plugin FX dentro de la cadena de una pista.
#[allow(dead_code)]
enum FxInst {
    #[cfg(feature = "clap")]
    Clap(Processor),
    #[cfg(feature = "vst3")]
    Vst3(vst3_slot::Vst3Slot),
}

#[cfg(feature = "clap")]
impl Drop for Renderer {
    fn drop(&mut self) {
        if let Some(tx) = &self.retire {
            for slot in self.plugins.iter_mut() {
                if let Some(p) = slot.take() { let _ = tx.send(p.into_stopped()); }
            }
            for chain in self.note_fx.iter_mut() {
                for p in chain.drain(..) { let _ = tx.send(p.into_stopped()); }
            }
            for chain in self.fx_chains.iter_mut() {
                for fx in chain.drain(..) {
                    match fx {
                        FxInst::Clap(p) => { let _ = tx.send(p.into_stopped()); }
                        #[cfg(feature = "vst3")]
                        FxInst::Vst3(_) => {}
                    }
                }
            }
        }
    }
}

impl Renderer {
    #[inline]
    fn has_plugin(&self, t: usize) -> bool {
        #[cfg(feature = "clap")]
        if self.plugins[t].is_some() { return true; }
        #[cfg(feature = "vst3")]
        if self.vst3_plugins[t].is_some() { return true; }
        #[cfg(not(any(feature = "clap", feature = "vst3")))]
        let _ = t;
        false
    }

    /// Nota hacia el plugin (evento time-stamped) o hacia el sinte interno.
    fn note(&mut self, t: usize, frame: u32, msg: MidiMsg) {
        let _ = frame; // solo se usa con la feature clap
        // CLAP *o* VST3: ambos leen la misma cola neutral `pev[t]`.
        // (Antes solo se miraba `plugins[t]` (CLAP): las notas de una pista
        // VST3 caian al sinte interno y el plugin VST3 nunca sonaba.)
        #[cfg(feature = "clap")]
        if self.has_plugin(t) {
            // Cola neutral: aqui ya no hay tipos de clack. El tope de eventos
            // lo aplica PluginEvents internamente.
            match msg {
                MidiMsg::On { note, vel, ch } => self.pev[t].note_on(frame, note, vel, ch),
                MidiMsg::Off { note, ch } => self.pev[t].note_off(frame, note, ch),
            }
            return;
        }
        match msg {
            MidiMsg::On { note, vel, .. } => self.synth.note_on(t as u8, note, vel),
            MidiMsg::Off { note, .. } => self.synth.note_off(t as u8, note),
        }
    }

    /// Tick dentro del loop del clip que se está grabando: si la pista tiene
    /// Tick relativo al origen de la grabación (o al playhead del clip activo).
    /// Así el primer ataque tras la frontera de quantize cae en 0.1 del clip,
    /// no en 2.1 del transporte global.
    fn rec_tick(&self, audio: &SessionAudio, t: usize) -> u32 {
        let lt = audio.rec_loop_ticks.max(1) as u64;
        if let Some(a) = self.engine.active(t) {
            return a.playhead % lt as u32;
        }
        let abs = self.engine.global_ticks().saturating_sub(self.rec_origin_ticks);
        (abs % lt) as u32
    }

    fn render(&mut self, out: &mut [f32], shared: &SessionShared) {
        let audio = shared.audio.latest();
        
        // Leer el snapshot POD publicado por GrooveKit, sin locks.
        self.caja_kit_live = shared.kit.mix_snapshot();

        // Parámetros de dado/síntesis y estado en vivo (seed, slot, locks).
        // Todo POD: copia por valor, sin locks ni asignaciones. Así el motor
        // de Caja ve el kit real en vez de los defaults con los que arrancó.
        let eng = shared.kit.engine_snapshot();
        let live = shared.kit.snapshot_live();
        for i in 0..NUM_PADS {
            let k = &mut self.caja_kit[i];
            k.cfg = eng[i].param;
            k.bank = eng[i].bank;
            k.poly = eng[i].poly;
            k.choke_group = eng[i].choke_group;
            k.choker = eng[i].choker;
            k.solo = eng[i].solo;
            self.caja_host[i] = (live[i].seed, live[i].slot, live[i].mode);
        }

        // Publicar la mezcla al motor usando arrays fijos.
        let mute = std::array::from_fn(|i| self.caja_kit_live[i].mute);
        let solo = std::array::from_fn(|i| self.caja_kit_live[i].solo);
        let pan = std::array::from_fn(|i| self.caja_kit_live[i].pan);
        let gain = std::array::from_fn(|i| self.caja_kit_live[i].gain);
        let rate = std::array::from_fn(|i| eng[i].rate);
        self.caja_engine.set_mixer(&mute, &solo, &pan, &gain, &rate);

        let n = audio.track_count as usize;
        let frames = (out.len() / self.channels.max(1)).min(MAX_BLOCK);
        let channels = self.channels;

        // Caja: recoge parámetros del kit una vez por bloque.
        self.caja_engine.begin_block(&self.caja_kit, &self.caja_host);

        for t in 0..n {
            let ta = &audio.tracks[t];
            let audible = !ta.mute && (!audio.any_solo || ta.solo);
            let ang = (ta.pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
            self.gains[t] = (ang.cos() * ta.volume, ang.sin() * ta.volume, audible);
            self.eq.bands[t].update(EqParams {
                sr: self.sample_rate as f32,
                low: ta.eq_low, mid: ta.eq_mid, high: ta.eq_high,
                mid_f: ta.eq_mid_f, mid_q: ta.eq_mid_q,
                hpf_on: ta.hpf_on, lpf_on: ta.lpf_on, hpf_hz: ta.hpf_hz, lpf_hz: ta.lpf_hz,
            });
            let buf = &mut self.bufs[t];
            buf[0][..frames].fill(0.0);
            buf[1][..frames].fill(0.0);
        }
        #[cfg(feature = "clap")]
        for t in 0..n { self.pev[t].clear(); }

        // ── Cambios de parámetros del plugin (Fase 3G) ──
        // TODO 3H: enviar cambios de parámetros al plugin. La API de
        // clack-extensions::params::host no está disponible públicamente
        // en la versión 0.2.0 (módulo privado). Por ahora los parámetros
        // se muestran en la UI pero no se envían al plugin.
        // La implementación correcta requiere callbacks asíncronos del
        // plugin que se agregarán en Fase 3H.

        // Al parar el transporte: soltar notas, volver a 1.1 y resetear el
        // scheduler de Groove Machine. Así el siguiente Play arranca el
        // loop de batería y los clips en el mismo downbeat (no en el
        // compás 2).
        if self.was_playing && !audio.playing {
            for t in 0..n {
                if self.has_plugin(t) {
                    if let Some(mv) = &mut self.voices[t].midi {
                        self.midi_out.clear();
                        mv.all_off(&mut self.midi_out);
                        for i in 0..self.midi_out.len { let m = self.midi_out.msgs[i]; self.note(t, 0, m); }
                    }
                } else { self.synth.all_off(t as u8); }
            }
            // Pausa (⏸) NO resetea el reloj: al volver a Play retoma en el
            // mismo compás, alineado al ciclo de Groove. Solo el stop global
            // (⏹ Todo) vuelve todo a 1.1.
            if STOP_ALL_REQ.swap(false, std::sync::atomic::Ordering::Relaxed) {
                self.engine.reset_transport();
                self.caja_scheduler.reset();
                for t in 0..n {
                    self.synth.all_off(t as u8);
                    self.sustain_down[t] = false;
                    self.sustained[t] = [false; 128];
                }
            }
        }
        self.was_playing = audio.playing;

        // Ciclo de Groove (para alinear grabación y lanzamientos al mismo fraseo).
        let groove_loop = if shared.caja_sched.enabled.load(std::sync::atomic::Ordering::Relaxed) {
            shared.caja_sched.schedule.try_lock()
                .map(|sch| sch.loop_ticks)
                .unwrap_or(self.engine.groove_loop_ticks)
        } else {
            0
        };
        self.engine.set_groove_loop_ticks(groove_loop);

        // Una grabación NUEVA pedida sin haber parado la anterior (pulsar otra ranura vacía de
        // una pista armada mientras se graba): este bloque se ve «parado», lo que cierra la toma
        // actual y reinicia la puerta de captura. El bloque siguiente ya espera la frontera.
        let forced_stop = audio.recording && self.rec_clip.is_some() && audio.rec_seq != self.rec_seq_seen;
        let audio_stopped;
        let audio: &SessionAudio = if forced_stop {
            self.rec_seq_seen = audio.rec_seq;
            audio_stopped = SessionAudio { recording: false, ..*audio };
            &audio_stopped
        } else {
            audio
        };
        if audio.recording && self.rec_clip.is_none() { self.rec_seq_seen = audio.rec_seq; }

        // Puerta de captura: quantize del clip alineado al ciclo de la batería.
        let mut rec_q = audio.rec_q_ticks;
        if rec_q > 0 && groove_loop > 0 {
            let gcd = |mut x: u64, mut y: u64| { while y != 0 { let t = y; y = x % y; x = t; } x.max(1) };
            let q = rec_q as u64;
            let period = gcd(audio.rec_loop_ticks.max(1) as u64, groove_loop as u64);
            rec_q = (q / gcd(q, period) * period) as u32;
        }
        let rec_live = rec_gate(
            audio.recording,
            audio.playing,
            rec_q,
            self.engine.global_ticks(),
        );
        // La identidad del clip que se graba se fija al empezar: el cursor de grabación
        // (rec_track / rec_scene) sigue a la selección y cambia, por ejemplo, al lanzar otro
        // clip de la pista.
        let armed_now = audio.recording && self.rec_clip.is_none();
        if armed_now {
            self.rec_clip = Some((audio.rec_track as usize, audio.rec_scene as usize));
        }
        // Feedback del estado de grabación hacia la GUI y los LEDs.
        if let Some((rt_t, rt_s)) = self.rec_clip {
            if audio.recording {
                if !rec_live {
                    shared.rt.set_clip_state(rt_t, rt_s, ClipState::RecQueued);
                } else if !self.rec_on {
                    shared.rt.set_clip_state(rt_t, rt_s, ClipState::Recording);
                }
            } else {
                // La grabación terminó. Si el cursor sigue en ese clip, se cerró con Rec y el
                // clip queda listo (Playing). Si el cursor se movió (se lanzó otro clip de la
                // pista) el clip grabado queda parado: antes el estado se escribía sobre el
                // clip equivocado y la GUI seguía mostrando «grabando».
                let displaced = (rt_t, rt_s) != (audio.rec_track as usize, audio.rec_scene as usize);
                let st = if displaced || !self.rec_on { ClipState::Empty } else { ClipState::Playing };
                shared.rt.set_clip_state(rt_t, rt_s, st);
            }
        }
        if self.rec_on && !rec_live {
            let (t, sc) = self.rec_clip.unwrap_or((audio.rec_track as usize, audio.rec_scene as usize));
            let now = self.rec_tick(audio, t);
            for slot in self.rec_pending.iter_mut() {
                if let Some((n, v, st)) = slot.take() {
                    let dur = if now >= st { now - st }
                              else { audio.rec_loop_ticks.saturating_sub(st) + now };
                    let _ = shared.rec_rx.push(RecNote { track: t as u8, scene: sc as u8, note: n, vel: v, tick: st, dur: dur.max(1) });
                }
            }
        }
        let old_rec_on = self.rec_on;
        self.rec_on = rec_live;
        if !audio.recording { self.rec_clip = None; }

        // Fijar origen de ticks al armar la grabación. La parada del clip
        // anterior solo ocurre cuando la puerta de captura se abre realmente
        // (rec_live), no durante el count-in/RecQueued: así se mantiene la
        // fluidez en vivo mientras se espera el 1.1 de grabación.
        if armed_now {
            let now = self.engine.global_ticks();
            self.rec_origin_ticks = if rec_q > 0 { (now / rec_q as u64) * rec_q as u64 } else { now };
        }
        let mut force_stop_track: Option<usize> = None;
        if rec_live && !old_rec_on {
            force_stop_track = Some(audio.rec_track as usize);
            let now = self.engine.global_ticks();
            self.rec_origin_ticks = if rec_q > 0 { (now / rec_q as u64) * rec_q as u64 } else { now };
        }

        // Borrados pedidos por la UI: matar el clip activo de esa pista de
        // inmediato, sin esperar la frontera de quantize. Array fijo, sin
        // alloc en RT; try_lock para no bloquear el callback.
        let mut kill = [0usize; MAX_TRACKS];
        let mut kill_n = 0usize;
        if let Ok(mut q) = KILL_TRACKS.try_lock() {
            for t in q.drain(..) {
                if kill_n < MAX_TRACKS && t < MAX_TRACKS {
                    kill[kill_n] = t;
                    kill_n += 1;
                }
            }
        }

        self.events.clear();
        for f in 0..frames {
            self.engine.process(audio, &shared.trig_rx, &mut self.events, 1);
            if f == 0 {
                for i in 0..kill_n {
                    self.engine.stop_track_immediate(kill[i], &mut self.events);
                    let kt = kill[i];
                    self.synth.all_off(kt as u8);
                    self.sustain_down[kt] = false;
                    self.sustained[kt] = [false; 128];
                    #[cfg(feature = "clap")]
                    if self.has_plugin(kt) { self.pev[kt].midi3(0, 0xB0, 123, 0); }
                }
            }
            if let Some(t) = force_stop_track.take() {
                self.engine.stop_track_immediate(t, &mut self.events);
            }

            // Recorremos por índice copiando el evento (ClipEvent es Copy):
            // así no mantenemos prestado self.events mientras llamamos a
            // métodos de self (has_plugin / note). Sin alloc.
            let mut ei = 0;
            while ei < self.events.len() {
                let ev = self.events[ei];
                ei += 1;
                if let ClipEvent::StateChange { track, scene, new_state } = ev {
                    let t = track as usize;
                    shared.rt.set_clip_state(t, scene as usize, new_state);
                    match new_state {
                        ClipState::Playing => {
                            self.voices[t].audio = shared.samples.load(t, scene as usize);
                            self.voices[t].midi = shared.notes.load(t, scene as usize).map(|data| {
                                let a = self.engine.active(t);
                                let (lt, ph) = a.map(|a| (a.loop_ticks, a.playhead)).unwrap_or((1920, 0));
                                MidiVoice::new(data, lt, ph)
                            });
                        }
                        ClipState::Empty => {
                            if self.has_plugin(t) {
                                if let Some(mv) = &mut self.voices[t].midi {
                                    self.midi_out.clear();
                                    mv.all_off(&mut self.midi_out);
                                    for i in 0..self.midi_out.len {
                                        let m = self.midi_out.msgs[i];
                                        self.note(t, f as u32, m);
                                    }
                                }
                            } else { self.synth.all_off(t as u8); }
                            self.voices[t].audio = None;
                            self.voices[t].midi = None;
                        }
                        _ => {}
                    }
                }
            }

            self.events.clear();

            // Notas del clip activo (playhead del motor).
            for t in 0..n {
                let Some(a) = self.engine.active(t) else { continue };
                let Some(mv) = &mut self.voices[t].midi else { continue };
                self.midi_out.clear();
                mv.advance(a.playhead, a.loop_ticks, &mut self.midi_out);
                for i in 0..self.midi_out.len { let m = self.midi_out.msgs[i]; self.note(t, f as u32, m); }
            }

            // Notas en vivo del teclado: suenan aunque el transporte esté parado.
            if n > 0 {
                for _ in 0..32 {
                    let Some(raw) = shared.live_rx.pop() else { break };
                    let Some(cmd) = ControlCmd::decode(raw) else { continue };
                    match cmd {
                        ControlCmd::LiveNoteOn { track, note, vel } => {
                            let t = (track as usize).min(n - 1);
                            // Si la nota estaba retenida por el pedal, se re-ataca limpia.
                            self.sustained[t][(note & 0x7F) as usize] = false;
                            self.note(t, f as u32, MidiMsg::On { note, vel, ch: 0 });
                            if rec_live && t == audio.rec_track as usize {
                                let tick = self.rec_tick(audio, t);
                                if let Some(slot) = self.rec_pending.iter_mut().find(|s| s.is_none()) {
                                    *slot = Some((note, vel, tick));
                                }
                            }
                        }
                        ControlCmd::LiveNoteOff { track, note } => {
                            let t = (track as usize).min(n - 1);
                            // Sinte interno con el pedal pisado: el note-off queda
                            // retenido hasta soltarlo. Los plugins reciben el CC 64 y
                            // aplican su propio sustain, así que ahí el off va directo.
                            if self.sustain_down[t] && !self.has_plugin(t) {
                                self.sustained[t][(note & 0x7F) as usize] = true;
                            } else {
                                self.note(t, f as u32, MidiMsg::Off { note, ch: 0 });
                            }
                            if rec_live && t == audio.rec_track as usize {
                                let now = self.rec_tick(audio, t);
                                let found = self.rec_pending.iter_mut()
                                    .find(|s| matches!(s, Some((nn, _, _)) if *nn == note));
                                if let Some(slot) = found {
                                    if let Some((nn, v, st)) = slot.take() {
                                        let dur = if now >= st { now - st }
                                                  else { audio.rec_loop_ticks.saturating_sub(st) + now };
                                        let _ = shared.rec_rx.push(RecNote {
                                            track: t as u8,
                                            scene: self.rec_clip.map(|(_, sc)| sc as u8).unwrap_or(audio.rec_scene as u8),
                                            note: nn, vel: v, tick: st, dur: dur.max(1),
                                        });
                                    }
                                }
                            }
                        }
                        ControlCmd::LiveSustain { track, value } => {
                            let t = (track as usize).min(n - 1);
                            let down = value >= 64;
                            if self.has_plugin(t) {
                                #[cfg(feature = "clap")]
                                self.pev[t].midi3(f as u32, 0xB0, 64, value);
                            } else {
                                let was_down = self.sustain_down[t];
                                self.sustain_down[t] = down;
                                if was_down && !down {
                                    // Pedal suelto: liberar todas las notas retenidas.
                                    for nn in 0..128usize {
                                        if self.sustained[t][nn] {
                                            self.sustained[t][nn] = false;
                                            self.note(t, f as u32, MidiMsg::Off { note: nn as u8, ch: 0 });
                                        }
                                    }
                                }
                            }
                        }
                        ControlCmd::LivePitchBend { track, value } => {
                            let t = (track as usize).min(n - 1);
                            if self.has_plugin(t) {
                                #[cfg(feature = "clap")]
                                self.pev[t].midi3(f as u32, 0xE0,
                                    (value & 0x7F) as u8, ((value >> 7) & 0x7F) as u8);
                            } else {
                                self.synth.pitch_bend(t as u8, value);
                            }
                        }
                        ControlCmd::LiveModWheel { track, value } => {
                            let t = (track as usize).min(n - 1);
                            if self.has_plugin(t) {
                                #[cfg(feature = "clap")]
                                self.pev[t].midi3(f as u32, 0xB0, 1, value);
                            } else {
                                self.synth.mod_wheel(t as u8, value);
                            }
                        }
                        ControlCmd::LivePadOn { pad, vel } => {
                            if (pad as usize) < NUM_PADS && self.caja_trig_count < 64 {
                                self.caja_trigs[self.caja_trig_count] =
                                    (f as u32, pad, vel as f32 / 127.0);
                                self.caja_trig_count += 1;
                            }
                        }
                        ControlCmd::LivePadOff { .. } => {} // Caja usa muestras one-shot
                        ControlCmd::SetPluginParam { track, param_idx, value } => {
                            #[cfg(feature = "clap")]
                            {
                                let t = (track as usize).min(n.saturating_sub(1));
                                if self.has_plugin(t) {
                                    if let Some((id, plain)) = crate::clap_params::param_plain(t, param_idx as usize, value) {
                                        // La cola guarda (id, plain). El Pckn neutro y el
                                        // ParamValueEvent se construyen al traducir, en
                                        // plugin_events::flush_clap.
                                        self.pev[t].param(f as u32, id, plain);
                                    }
                                }
                            }

                            #[cfg(not(feature = "clap"))]
                            let _ = (track, param_idx, value);
                        }
                        _ => {}
                    }
                }
            }

            self.synth.advance();
            let spt = self.engine.samples_per_tick();
            for t in 0..n {
                let mut l = 0.0f32;
                let mut r = 0.0f32;
                if !self.has_plugin(t) { l = self.synth.track_sum(t); r = l; }
                if let Some(sd) = &self.voices[t].audio {
                    if let (Some(a), Some(pos)) = (self.engine.active(t), self.engine.phase_frames(t)) {
                        let ch = sd.channels.max(1) as usize;
                        let df = sd.data.len() / ch;
                        if df > 0 {
                            let loop_frames = (a.loop_ticks as f64 * spt).max(1.0);
                        // Loop parcial: empieza en loop_start_sec y envuelve al llegar al final
                        // del archivo. Asi el clip puede loopear menos de lo que dura el WAV.
                        let start_f = ((a.loop_start_sec as f64 * sd.sample_rate as f64) as usize)
                            .min(df.saturating_sub(1));
                        let eff = loop_frames.min((df - start_f) as f64).max(1.0);
                        let p = start_f as f64 + (pos % eff);
                        let i0 = (p as usize).min(df - 1);
                            let i1 = (i0 + 1) % df;
                            let fr = (p - i0 as f64) as f32;
                            if ch == 1 {
                                let s = sd.data[i0] + (sd.data[i1] - sd.data[i0]) * fr;
                                l += s; r += s;
                            } else {
                                l += sd.data[i0 * ch] + (sd.data[i1 * ch] - sd.data[i0 * ch]) * fr;
                                r += sd.data[i0 * ch + 1] + (sd.data[i1 * ch + 1] - sd.data[i0 * ch + 1]) * fr;
                            }
                        }
                    }
                }
                let buf = &mut self.bufs[t];
                buf[0][f] += l;
                buf[1][f] += r;
            }
        }

        // ── Note FX CLAP: transforman las notas de la pista ANTES del instrumento ──
        // Entran las notas/MIDI crudo de pev[t]; los Param se conservan intactos. La
        // salida del plugin reemplaza las notas en pev[t] (ordenada por frame), así
        // que el instrumento (CLAP o VST3) ya recibe las notas transformadas en el
        // mismo bloque: latencia añadida = 0. Buffers fijos, sin alloc ni locks.
        #[cfg(feature = "clap")]
        for t in 0..n {
            if self.note_fx[t].is_empty() || !self.has_plugin(t) { continue; }
            for ci in 0..self.note_fx[t].len() {
            let Some(proc) = self.note_fx[t].get_mut(ci) else { continue };
            let Ok(started) = proc.ensure_processing_started() else { continue };
            self.nfx_src.clear();
            self.nfx_keep.clear();
            for e in self.pev[t].as_slice() {
                if e.kind == crate::plugin_events::EvKind::Param { self.nfx_keep.push_ev(*e); }
                else { self.nfx_src.push_ev(*e); }
            }
            crate::plugin_events::flush_clap(&self.nfx_src, &mut self.nfx_in);
            self.nfx_out.clear();
            let ok = {
                // Un plugin de notas no tiene audio, pero process() deduce el número
                // de frames de los buffers: se le da el mismo puerto de salida de
                // trabajo que a los instrumentos y se descarta.
                let [pl, pr] = &mut *self.pbuf;
                pl[..frames].fill(0.0);
                pr[..frames].fill(0.0);
                let mut audio_out = self.out_ports.with_output_buffers([AudioPortBuffer {
                    latency: 0,
                    channels: AudioPortBufferType::f32_output_only(
                        [&mut pl[..frames], &mut pr[..frames]].into_iter()
                    ),
                }]);
                let input_events = self.nfx_in.as_input();
                let mut output_events = self.nfx_out.as_output();
                started.process(
                    &InputAudioBuffers::empty(), &mut audio_out,
                    &input_events, &mut output_events,
                    Some(self.steady), None,
                ).is_ok()
            };
            if !ok { continue; } // si el plugin falla, las notas pasan sin transformar
            self.pev[t].clear();
            for e in self.nfx_keep.as_slice() { self.pev[t].push_ev(*e); }
            crate::plugin_events::read_clap(&self.nfx_out, &mut self.pev[t]);
            self.pev[t].sort_by_frame();
            } // cadena Note FX
        }

        // ── Plugins VST3: un process_bus_audio() por pista y bloque ──
        // Efectos: la señal de la pista entra por los buses de audio del plugin
        // y la salida sustituye al dry (no se suma dos veces). Instrumentos:
        // generan desde MIDI y su salida se suma a la pista.
        #[cfg(feature = "vst3")]
        for t in 0..n {
            let Some(slot) = self.vst3_plugins[t].as_mut() else { continue; };
            let is_fx = !slot.is_instrument;
            let (in_l, in_r): (&[f32], &[f32]) = if is_fx {
                (&self.bufs[t][0][..frames], &self.bufs[t][1][..frames])
            } else {
                (&[], &[])
            };
            if !vst3_slot::process(slot, self.pev[t].as_slice(), frames, in_l, in_r) { continue; }

            if !slot.buffers.outputs.is_empty() {
                let out_bus = &slot.buffers.outputs[0];
                let buf = &mut self.bufs[t];
                if is_fx {
                    for f in 0..frames { buf[0][f] = 0.0; buf[1][f] = 0.0; }
                }
                let ch_count = out_bus.channels.len().min(2);
                for f in 0..frames {
                    if ch_count >= 1 { buf[0][f] += out_bus.channels[0][f]; }
                    if ch_count >= 2 { buf[1][f] += out_bus.channels[1][f]; }
                    else             { buf[1][f] += out_bus.channels[0][f]; }
                }
            }
        }

        // ── Plugins CLAP: un process() por pista y bloque ──
        #[cfg(feature = "clap")]
        for t in 0..n {
            if self.plugins[t].is_none() { continue; }
            let Some(proc) = self.plugins[t].as_mut() else { continue };
            let Ok(started) = proc.ensure_processing_started() else { continue };
            {
                let [pl, pr] = &mut *self.pbuf;
                pl[..frames].fill(0.0);
                pr[..frames].fill(0.0);
                let mut audio_out = self.out_ports.with_output_buffers([AudioPortBuffer {
                    latency: 0,
                    channels: AudioPortBufferType::f32_output_only(
                        [&mut pl[..frames], &mut pr[..frames]].into_iter()
                    ),
                }]);
                // Traduccion cola neutral -> CLAP. Sin alloc: EventBuffer ya
                // tiene MAX_EVENTS reservado. Unico punto que conoce el formato.
                crate::plugin_events::flush_clap(&self.pev[t], &mut self.ev[t]);
                let input_events = self.ev[t].as_input();
                let mut output_events = OutputEvents::void();
                let _ = started.process(
                    &InputAudioBuffers::empty(), &mut audio_out,
                    &input_events, &mut output_events,
                    Some(self.steady), None,
                );
            }
            let buf = &mut self.bufs[t];
            for f in 0..frames {
                buf[0][f] += self.pbuf[0][f];
                buf[1][f] += self.pbuf[1][f];
            }
        }
        // ── FX de audio por pista (insert después del instrumento y de los clips de
        // audio): la mezcla de la pista entra al plugin y su salida la sustituye.
        // Sirve igual para pistas MIDI (salida del instrumento) y de audio.
        for t in 0..n {
            for ci in 0..self.fx_chains[t].len() {
                match &mut self.fx_chains[t][ci] {
                    #[cfg(feature = "vst3")]
                    FxInst::Vst3(slot) => {
                        if !vst3_slot::process(slot, &[], frames, &self.bufs[t][0][..frames], &self.bufs[t][1][..frames]) {
                            continue;
                        }
                        if slot.buffers.outputs.is_empty() { continue; }
                        let out_bus = &slot.buffers.outputs[0];
                        let ch_count = out_bus.channels.len().min(2);
                        if ch_count == 0 { continue; }
                        let buf = &mut self.bufs[t];
                        for f in 0..frames {
                            let l = out_bus.channels[0][f];
                            let r = if ch_count >= 2 { out_bus.channels[1][f] } else { l };
                            buf[0][f] = l;
                            buf[1][f] = r;
                        }
                    }
                    #[cfg(feature = "clap")]
                    FxInst::Clap(proc) => {
                        let Ok(started) = proc.ensure_processing_started() else { continue };
                        let ok = {
                            let [il, ir] = &mut *self.fxin;
                            il[..frames].copy_from_slice(&self.bufs[t][0][..frames]);
                            ir[..frames].copy_from_slice(&self.bufs[t][1][..frames]);
                            let [pl, pr] = &mut *self.pbuf;
                            pl[..frames].fill(0.0);
                            pr[..frames].fill(0.0);
                            let in_bufs = self.fx_ports.with_input_buffers([AudioPortBuffer {
                                latency: 0,
                                channels: AudioPortBufferType::f32_input_only([
                                    InputChannel::variable(&mut il[..frames]),
                                    InputChannel::variable(&mut ir[..frames]),
                                ].into_iter()),
                            }]);
                            let mut audio_out = self.out_ports.with_output_buffers([AudioPortBuffer {
                                latency: 0,
                                channels: AudioPortBufferType::f32_output_only(
                                    [&mut pl[..frames], &mut pr[..frames]].into_iter()
                                ),
                            }]);
                            self.nfx_in.clear();
                            let input_events = self.nfx_in.as_input();
                            let mut output_events = OutputEvents::void();
                            started.process(
                                &in_bufs, &mut audio_out,
                                &input_events, &mut output_events,
                                Some(self.steady), None,
                            ).is_ok()
                        };
                        if !ok { continue; } // si el efecto falla, ese slot no altera la señal
                        let buf = &mut self.bufs[t];
                        for f in 0..frames {
                            buf[0][f] = self.pbuf[0][f];
                            buf[1][f] = self.pbuf[1][f];
                        }
                    }
                    #[cfg(not(any(feature = "clap", feature = "vst3")))]
                    _ => {}
                }
            }
        }
        #[cfg(feature = "clap")]
        { self.steady = self.steady.wrapping_add(frames as u64); }

        // ── Motor de drums de Caja: render por segmentos entre golpes ──
        {
            // Llamar al scheduler para obtener triggers automáticos del patrón
            // Posición musical del launcher (fuente de verdad para toda la sesión).
            // Convierte ticks a beats: 1920 ticks = 4 beats = 1 compás 4/4.
            // OJO: el motor de clips ya avanzó `frames` ticks dentro de este bloque
            // (bucle por frame de arriba). Groove espera la posición del INICIO del
            // bloque; antes recibía la del final y sonaba adelantado un buffer
            // respecto a los clips.
            let spt = self.engine.samples_per_tick().max(1.0);
            let inicio_ticks = self.engine.position_ticks() - frames as f64 / spt;
            let song_beats = inicio_ticks * 4.0 / TICKS_PER_BAR as f64;
            // Publicar el ciclo de Groove al motor de clips: así el
            // scene barrier y effective_quantize_ticks alinean los
            // lanzamientos al downbeat real de la batería.
            let gt_now = self.engine.global_ticks();
            if gt_now < self.last_global_ticks { self.caja_scheduler.reset(); }
            self.last_global_ticks = gt_now;
            let glt = self.caja_scheduler.groove_loop_ticks();
            self.engine.set_groove_loop_ticks(glt);

            let sched_trigs = self.caja_scheduler.process(frames, audio.playing, audio.tempo as f64, song_beats);
            
            // Fusionar triggers manuales (pads físicos) + automáticos (scheduler)
            for i in 0..64 {
                let (at, pad, vel) = sched_trigs[i];
                if vel > 0.0 && self.caja_trig_count < 64 {
                    self.caja_trigs[self.caja_trig_count] = (at, pad, vel);
                    self.caja_trig_count += 1;
                }
            }
            
            // Posición musical en beats: la necesita el re-dado automático
            // (rate = cada N compases) dentro de CajaEngine::trigger.
            let song_beats =
                self.engine.global_ticks() as f64 * 4.0 / TICKS_PER_BAR as f64;

            // Desestructurar el buffer para tener dos préstamos mutables
            // independientes (L y R) sin conflicto con el borrow checker.
            let [cb_l, cb_r] = &mut *self.caja_buf;
            cb_l[..frames].fill(0.0);
            cb_r[..frames].fill(0.0);
            let trigs = &mut self.caja_trigs[..self.caja_trig_count];
            trigs.sort_unstable_by_key(|t| t.0);
            let mut pos = 0usize;
            for &(at, pad, vel) in trigs.iter() {
                let at = (at as usize).min(frames);
                if at > pos {
                    self.caja_engine.render(&mut cb_l[pos..at], &mut cb_r[pos..at]);
                    pos = at;
                }
                let kit = &self.caja_kit;
                let store = &self.caja_store;
                // CajaEngine aplica la mezcla publicada mediante set_mixer().
                self.caja_engine.trigger(pad as usize, vel, kit, store, song_beats);
            }
            if pos < frames {
                self.caja_engine.render(&mut cb_l[pos..frames], &mut cb_r[pos..frames]);
            }
            self.caja_trig_count = 0;

            // ── FX globales del bus Groove (PITCH / MOD / PERFORM) ──
            let fx_pitch = audio.groove_fx_pitch.clamp(0.0, 1.0);
            let fx_filter = audio.groove_fx_filter.clamp(0.0, 1.0);
            let fx_perform = audio.groove_fx_perform.clamp(0.0, 1.0);
            let sr = self.sample_rate.max(1) as f32;

            // PITCH: 0.5 = neutro. <0.5 = tape stop hacia 0; >0.5 = varispeed up.
            let pitch_target = if (fx_pitch - 0.5).abs() < 0.01 {
                1.0
            } else if fx_pitch < 0.5 {
                (fx_pitch / 0.5).clamp(0.0, 1.0)
            } else {
                1.0 + (fx_pitch - 0.5) * 2.0 // hasta 2x
            };
            // Tape stop feel: slew env toward target; when going down hard, ramp to stop.
            let tape_slew = if pitch_target < self.groove_fx_tape_env { 0.0025 } else { 0.02 };
            // Apply varispeed/tape by resampling in-place via scratch copy.
            if (pitch_target - 1.0).abs() > 0.001 || (self.groove_fx_tape_env - 1.0).abs() > 0.001 {
                // copy to temp fixed stack arrays? MAX_BLOCK is 4096; two [f32;4096] on stack is ok.
                let mut tmp_l = [0.0f32; MAX_BLOCK];
                let mut tmp_r = [0.0f32; MAX_BLOCK];
                tmp_l[..frames].copy_from_slice(&cb_l[..frames]);
                tmp_r[..frames].copy_from_slice(&cb_r[..frames]);
                let mut posf = 0.0f32;
                for i in 0..frames {
                    self.groove_fx_tape_env += (pitch_target - self.groove_fx_tape_env) * tape_slew;
                    let rate = self.groove_fx_tape_env.clamp(0.0, 2.0);
                    let idx = posf as usize;
                    let frac = posf - idx as f32;
                    let s0 = idx.min(frames.saturating_sub(1));
                    let s1 = (idx + 1).min(frames.saturating_sub(1));
                    cb_l[i] = tmp_l[s0] + (tmp_l[s1] - tmp_l[s0]) * frac;
                    cb_r[i] = tmp_r[s0] + (tmp_r[s1] - tmp_r[s0]) * frac;
                    posf += rate;
                    if posf >= frames as f32 { 
                        // reached end: fill rest with tail/fade
                        for j in i+1..frames { cb_l[j] = 0.0; cb_r[j] = 0.0; }
                        break;
                    }
                }
            } else {
                self.groove_fx_tape_env = 1.0;
            }

            // MOD: resonant filter sweep LPF<->HPF. 0.5 neutral bypass.
            if (fx_filter - 0.5).abs() > 0.01 {
                let q = 1.2;
                if fx_filter < 0.5 {
                    // LPF: 0.0 very closed, 0.5 open
                    let t = fx_filter / 0.5;
                    let hz = 80.0 * (18000.0f32 / 80.0f32).powf(t.clamp(0.0, 1.0));
                    self.groove_fx_filter_l.set_lpf(sr, hz, q);
                    self.groove_fx_filter_r.set_lpf(sr, hz, q);
                } else {
                    // HPF: 0.5 off-ish, 1.0 aggressive
                    let t = (fx_filter - 0.5) / 0.5;
                    let hz = 20.0 * (8000.0f32 / 20.0f32).powf(t.clamp(0.0, 1.0));
                    self.groove_fx_filter_l.set_hpf(sr, hz, q);
                    self.groove_fx_filter_r.set_hpf(sr, hz, q);
                }
                for i in 0..frames {
                    cb_l[i] = self.groove_fx_filter_l.process(cb_l[i]);
                    cb_r[i] = self.groove_fx_filter_r.process(cb_r[i]);
                }
            } else {
                self.groove_fx_filter_l.reset();
                self.groove_fx_filter_r.reset();
            }

            // PERFORM: delay/stutter send synced to tempo. 0=no send.
            if fx_perform > 0.001 {
                let bpm = audio.tempo.max(1.0) as f32;
                // choose division by amount: low=1/8, mid=1/16, high=1/32
                let div = if fx_perform < 0.34 { 0.5 } else if fx_perform < 0.67 { 0.25 } else { 0.125 }; // beats
                let delay_samples = ((60.0 / bpm) * div * sr) as usize;
                let delay_samples = delay_samples.clamp(1, 131071);
                let send = fx_perform.clamp(0.0, 1.0) * 0.65;
                let fb = (0.2 + fx_perform * 0.45).clamp(0.0, 0.72);
                let mask = 131071usize;
                for i in 0..frames {
                    let rpos = (self.groove_fx_delay_w + 131072 - delay_samples) & mask;
                    let dl = self.groove_fx_delay_l[rpos];
                    let dr = self.groove_fx_delay_r[rpos];
                    let inl = (cb_l[i] + dl * fb).clamp(-2.0, 2.0);
                    let inr = (cb_r[i] + dr * fb).clamp(-2.0, 2.0);
                    self.groove_fx_delay_l[self.groove_fx_delay_w] = inl;
                    self.groove_fx_delay_r[self.groove_fx_delay_w] = inr;
                    self.groove_fx_delay_w = (self.groove_fx_delay_w + 1) & mask;
                    cb_l[i] += dl * send;
                    cb_r[i] += dr * send;
                }
            } else {
                // decay feedback tails
                let mut any = false;
                for s in self.groove_fx_delay_l.iter_mut() { *s *= 0.95; if s.abs() > 1e-5 { any = true; } }
                for s in self.groove_fx_delay_r.iter_mut() { *s *= 0.95; if s.abs() > 1e-5 { any = true; } }
                if !any { self.groove_fx_delay_w = 0; }
            }

            // Soft clip suave post-FX para evitar clipping al mover PITCH/MOD/PERFORM.
            for i in 0..frames {
                let l = cb_l[i];
                let r = cb_r[i];
                cb_l[i] = l / (1.0 + l.abs() * 0.35);
                cb_r[i] = r / (1.0 + r.abs() * 0.35);
            }
        }

        // ── EQ + mezcla final + metrónomo ──
        // ticks por sample: (BPM/60) * (PPQ) / sr  con PPQ = TICKS_PER_BAR/4
        let ticks_per_sample = if audio.playing && self.sample_rate > 0 {
            (audio.tempo / 60.0) * (TICKS_PER_BAR as f64 / 4.0) / self.sample_rate as f64
        } else {
            0.0
        };
        let beat_ticks = TICKS_PER_BAR as f64 / 4.0;
        let mut tick_f = self.engine.global_ticks() as f64;
        // Fase residual del click (decay en samples)
        // Se genera un impulso corto en cada frontera de beat.
        for f in 0..frames {
            let mut ml = 0.0f32;
            let mut mr = 0.0f32;
            for t in 0..n {
                let (gl, gr, audible) = self.gains[t];
                if !audible { continue; }
                // Mono: suma L+R (potencia constante) en ambos lados, para una entrada mono
                // que llegó a un solo canal.
                let (in_l, in_r) = if audio.tracks[t].mono {
                    let m = (self.bufs[t][0][f] + self.bufs[t][1][f]) * std::f32::consts::FRAC_1_SQRT_2;
                    (m, m)
                } else {
                    (self.bufs[t][0][f], self.bufs[t][1][f])
                };
                let l = self.eq.bands[t].process(0, in_l);
                let r = self.eq.bands[t].process(1, in_r);
                ml += l * gl;
                mr += r * gr;
                let pk = l.abs().max(r.abs()) * gl.max(gr);
                if pk > self.peaks[t] { self.peaks[t] = pk; }
            }
            // El Groove tiene su propio master; el master general se aplica después a todo.
            let gl = self.caja_buf[0][f] * audio.groove_master;
            let gr = self.caja_buf[1][f] * audio.groove_master;
            ml += gl;
            mr += gr;
            let gp = gl.abs().max(gr.abs());
            if gp > self.groove_peak { self.groove_peak = gp; }

            // Metrónomo: configurable (siempre / solo count-in de rec / off) y subdivisión.
            let rec_count_in = audio.recording
                && (0..audio.track_count as usize).any(|ti| {
                    self.engine.active(ti).map(|a| a.scene == audio.rec_scene && a.state == ClipState::RecQueued).unwrap_or(false)
                });
            let metro_active = match audio.metronome_mode {
                1 => rec_count_in, // Solo count-in de grabación
                2 => false,        // Off
                _ => audio.metronome && audio.playing, // Siempre (cuando está on y sonando)
            };
            if metro_active && ticks_per_sample > 0.0 {
                let prev = tick_f;
                tick_f += ticks_per_sample;
                let subdiv_ticks = match audio.metronome_subdiv {
                    2 => beat_ticks / 4.0, // 1/16
                    3 => beat_ticks / 8.0, // 1/32
                    1 => beat_ticks / 2.0, // 1/8
                    _ => beat_ticks,       // 1/4
                }.max(1.0);
                let prev_step = (prev / subdiv_ticks).floor();
                let cur_step = (tick_f / subdiv_ticks).floor();
                if cur_step > prev_step {
                    let is_bar = ((cur_step as f64 * subdiv_ticks) % (beat_ticks * 4.0)).abs() < 1.0;
                    let is_beat = ((cur_step as f64 * subdiv_ticks) % beat_ticks).abs() < 1.0;
                    self.metro_env = if is_bar { 0.32 } else if is_beat { 0.20 } else { 0.11 };
                    self.metro_freq = if is_bar { 2200.0 } else if is_beat { 1400.0 } else { 900.0 };
                    self.metro_phase = 0.0;
                }
            } else if ticks_per_sample > 0.0 {
                tick_f += ticks_per_sample;
            }
            if self.metro_env > 0.001 {
                let s = self.metro_phase.sin() * self.metro_env;
                ml += s;
                mr += s;
                self.metro_phase += self.metro_freq * std::f32::consts::TAU / self.sample_rate as f32;
                if self.metro_phase > std::f32::consts::TAU { self.metro_phase -= std::f32::consts::TAU; }
                // ~8 ms de decay
                self.metro_env *= 0.92;
            } else {
                self.metro_env = 0.0;
            }

            ml *= audio.master_volume;
            mr *= audio.master_volume;
            let mp = ml.abs().max(mr.abs());
            if mp > self.peaks[MAX_TRACKS] { self.peaks[MAX_TRACKS] = mp; }
            let base = f * channels;
            if channels == 1 { out[base] = (ml + mr) * 0.5; }
            else {
                out[base] = ml;
                out[base + 1] = mr;
                for c in 2..channels { out[base + c] = 0.0; }
            }
        }

        for t in 0..n {
            shared.rt.set_peak(t, self.peaks[t]);
            self.peaks[t] = 0.0;
            if let Some(a) = self.engine.active(t) {
                shared.rt.set_playhead(t, a.playhead);
                shared.rt.set_active_scene(t, a.scene);
            } else { shared.rt.set_active_scene(t, 0xFF); }
        }
        shared.rt.set_peak(MAX_TRACKS, self.peaks[MAX_TRACKS]);
        self.peaks[MAX_TRACKS] = 0.0;
        shared.rt.set_groove_peak(self.groove_peak);
        self.groove_peak = 0.0;

        let any_playing = (0..audio.track_count as usize).any(|t| self.engine.active(t).is_some());
        // ── Input monitoring (Fase F2) ──
        // Drena el ring buffer de entrada y mezcla al output.
        // Si el input tiene más o menos canales que el output, se mapea el
        // primer canal de input a todos los canales de output (mono).
        if let (Some(cons), in_ch) = (&mut self.audio_in_cons, self.audio_in_ch as usize) {
            if in_ch > 0 && frames > 0 {
                let needed = frames * in_ch;
                if self.audio_in_scratch.len() != needed {
                    self.audio_in_scratch.resize(needed, 0.0);
                }
                // Leer exactamente `needed` muestras del ring buffer.
                // pop_slice devuelve un slice contiguo; si el ring es circular,
                // puede devolver menos de lo pedido, así que iteramos.
                let mut read = 0;
                while read < needed {
                    let slice = &mut self.audio_in_scratch[read..needed];
                    let got = cons.pop_slice(slice);
                    if got == 0 {
                        break;
                    }
                    read += got;
                }
                // Rellenar con ceros si faltan muestras (input más lento que output).
                for i in read..needed {
                    self.audio_in_scratch[i] = 0.0;
                }
                // Monitoreo por pista ARMADA y por su canal de entrada.
                let _ = any_playing;
                let out_ch = self.channels as usize;
                let mon_gain = 0.8f32;
                for t in 0..n {
                    let ta = &audio.tracks[t];
                    if !ta.arm { continue; }
                    let (gl, gr, audible) = self.gains[t];
                    if !audible { continue; }
                    // Si su clip ya suena, se oye el clip (no el instrumento en vivo).
                    if self.engine.active(t).is_some() { continue; }
                    let ch_t = (ta.input_ch as usize).min(in_ch - 1);
                    let mut pk = 0.0f32;
                    for f in 0..frames {
                        let s = self.audio_in_scratch[f * in_ch + ch_t] * mon_gain;
                        let l = s * gl * audio.master_volume;
                        let r = s * gr * audio.master_volume;
                        let base = f * out_ch;
                        if out_ch == 1 {
                            if base < out.len() { out[base] += (l + r) * 0.5; }
                        } else if base + 1 < out.len() {
                            out[base] += l;
                            out[base + 1] += r;
                        }
                        let a = s.abs() * gl.max(gr);
                        if a > pk { pk = a; }
                    }
                    if pk > self.peaks[t] { self.peaks[t] = pk; }
                }
                // El pico del input ya se contabiliza por pista en el monitoreo.
            }
        }

        // ── RecordEngine (Fase F3) ──
        // Detectar cambios en audio.recording y arrancar/parar slots.
        if self.rec_on && !self.was_recording {
            let rt = audio.rec_track as usize;
            let rs = audio.rec_scene as usize;
            self.rec_audio_slot = Some((rt, rs));
            self.record_engine
                .start_recording(rt, rs, 1, self.sample_rate);
            println!("[record] inicio: pista {} escena {}", rt + 1, rs + 1);
        } else if !self.rec_on && self.was_recording {
            // La toma se cierra en el clip donde EMPEZÓ, aunque el cursor se haya movido.
            let (rt, rs) = self.rec_audio_slot.take()
                .unwrap_or((audio.rec_track as usize, audio.rec_scene as usize));
            if let Some(clip) = self.record_engine.stop_recording(rt, rs) {
                let denom = (clip.sample_rate as usize * clip.channels as usize).max(1);
                let secs = clip.samples.len() / denom;
                println!(
                    "[record] fin: pista {} escena {} - {} muestras ({} s)",
                    rt + 1, rs + 1, clip.samples.len(), secs
                );
                // RT: SOLO deposita el clip ya asignado en el slot global.
                // NO hay I/O ni alloc aquí: el loader escribe el WAV fuera.
                // Handoff inspirado en Maolan (BSD-2) y MAGDA (GPL-3):
                // el disco nunca se toca en el callback de audio.
                let mut slot = REC_SLOT.lock().unwrap_or_else(|e| e.into_inner());
                *slot = Some((rt, rs, clip));
            }
        }
        self.was_recording = self.rec_on;

        // Escribir muestras en todos los slots activos.
        if self.record_engine.is_any_recording() {
            // Se graba SOLO el canal de entrada de la pista que graba (clip mono).
            let in_ch = self.audio_in_ch as usize;
            if in_ch > 0 && self.audio_in_scratch.len() >= frames * in_ch {
                let rt = self.rec_clip.map(|(t, _)| t).unwrap_or(audio.rec_track as usize);
                let ch_t = (audio.tracks[rt.min(n.saturating_sub(1))].input_ch as usize).min(in_ch - 1);
                if self.rec_mono.len() != frames { self.rec_mono.resize(frames, 0.0); }
                for f in 0..frames { self.rec_mono[f] = self.audio_in_scratch[f * in_ch + ch_t]; }
                self.record_engine.write(&self.rec_mono[..frames]);
            }
        }
    }
}

pub fn start(shared: Arc<SessionShared>) -> Result<AudioHandle, String> {
    crate::clap_gui::install_alarm_handler();
    // yabridge deduce el prefijo wine de la ruta de cada plugin. Un WINEPREFIX
    // heredado del shell lo fuerza a uno equivocado (plugin en .wine-arturia pero
    // wine en .wine) -> IComponent::initialize falla o crashea. Lo quitamos para
    // que funcione igual desde terminal que desde el menú.
    // Escape: LANZADERA_KEEP_WINEPREFIX=1 conserva la variable.
    if std::env::var_os("LANZADERA_KEEP_WINEPREFIX").is_none() {
        if let Some(v) = std::env::var_os("WINEPREFIX") {
            println!("[lanzadera] quitando WINEPREFIX={:?} (yabridge auto-detecta por plugin)", v);
            std::env::remove_var("WINEPREFIX");
        }
    }
    #[cfg(feature = "jack")]
    {
        if let Ok(h) = cpal::host_from_id(cpal::HostId::Jack) {
            if let Ok(a) = build_for_host(h, shared.clone()) {
                println!("[lanzadera] audio: JACK (PipeWire si se lanzó con pw-jack)");
                return Ok(a);
            }
        }
    }
    let a = build_for_host(cpal::default_host(), shared)?;
    println!("[lanzadera] audio: ALSA/default @ {} Hz", a.sample_rate);
    Ok(a)
}

fn build_for_host(host: cpal::Host, shared: Arc<SessionShared>) -> Result<AudioHandle, String> {
    let device = host.default_output_device().ok_or("sin dispositivo de salida")?;
    let supported = device.default_output_config().map_err(|e| e.to_string())?;
    let channels = supported.channels() as usize;
    // Alinear la app a la tasa del grafo PipeWire (48000) para evitar remuestreo
    // por bloque, que bajo pipewire-alsa es una fuente tipica de xruns.
    // Override manual: LANZADERA_RATE=48000 (o la tasa que quieras forzar).
    let sample_rate = {
        let default_rate = supported.sample_rate().0;
        match std::env::var("LANZADERA_RATE").ok().and_then(|v| v.parse::<u32>().ok()) {
            Some(r) if (8000..=384000).contains(&r) => r,
            _ => {
                let has48 = device.supported_output_configs().ok()
                    .map(|mut it| it.any(|c| c.min_sample_rate().0 <= 48000 && 48000 <= c.max_sample_rate().0))
                    .unwrap_or(false);
                if has48 && default_rate != 48000 {
                    println!("[lanzadera] usando 48000 Hz (tasa del grafo) en vez de {} Hz", default_rate);
                    48000
                } else {
                    default_rate
                }
            }
        }
    };
    // El Default de ALSA suele ser grande y añade decenas de ms entre pulsar
    // una tecla y oírla. LANZADERA_BUFFER=256 (frames); 0 = Default del driver.
    let want = std::env::var("LANZADERA_BUFFER").ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(256);
    // En JACK/PipeWire el buffer lo fija el servidor (PIPEWIRE_LATENCY).
    // HostId::Jack solo existe con la feature "jack" de cpal.
    #[cfg(feature = "jack")]
    let is_jack = host.id() == cpal::HostId::Jack;
    #[cfg(not(feature = "jack"))]
    let is_jack = false;
    let buffer_size = if want == 0 || is_jack {
        cpal::BufferSize::Default
    } else {
        match supported.buffer_size() {
            cpal::SupportedBufferSize::Range { min, max } => {
                cpal::BufferSize::Fixed(want.clamp(*min, *max))
            }
            cpal::SupportedBufferSize::Unknown => cpal::BufferSize::Fixed(want),
        }
    };
    if let cpal::BufferSize::Fixed(n) = buffer_size {
        println!("[lanzadera] buffer: {} frames (~{:.1} ms)", n, n as f64 * 1000.0 / sample_rate as f64);
    }
    if is_jack {
        println!("[lanzadera] JACK/PipeWire: quantum del servidor (PIPEWIRE_LATENCY)");
    }
    let cfg = cpal::StreamConfig {
        channels: channels as u16,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size,
    };

    #[allow(unused_mut)] // sin las features clap/vst3 no se añade nada
    let mut loaded: Vec<(usize, u8, usize, String)> = Vec::new();

    // Estado de GUI limpio en cada (re)arranque: si se quito un plugin, su
    // boton "Interfaz" no debe quedar activo.
    #[cfg(feature = "clap")]
    for t in 0..crate::clap_gui::MAX_PLUGIN_SLOTS {
        crate::clap_gui::set_avail(t, false);
        crate::clap_gui::set_open(t, false);
    }

    #[cfg(feature = "clap")]
    let (mut plugins, instances, retire_tx, retire_rx) = {
        let mut plugins: [Option<Processor>; MAX_TRACKS] = std::array::from_fn(|_| None);
        let mut instances = Vec::new();
        let (tx, rx) = std::sync::mpsc::channel();
        let plugs: Vec<(usize, String, Option<String>, Option<String>)> = {
            let st = shared.state.lock().unwrap();
            st.tracks.iter().enumerate()
                .filter_map(|(i, tr)| tr.plugin_path.clone().map(|p| (i, p, tr.plugin_id.clone(), tr.plugin_state.clone())))
                .collect()
        };
        for (t, path, id, state) in plugs {
            if path.ends_with(".vst3") { continue; } // lo carga el bloque VST3
            match clap_host::load(&path, id.as_deref(), sample_rate, MAX_BLOCK as u32, t) {
                Ok((mut inst, proc, name, params)) => {
                    println!(
                        "[lanzadera] CLAP pista {}: {} · {} parámetros",
                        t + 1,
                        name,
                        params.len()
                    );

                    // Tabla RT: param_idx -> clap_param_id.
                    crate::clap_params::set_track_params(t, &params);

                    // Estado UI: (id_string, nombre, valor normalizado).
                    let ui_params: Vec<(String, String, f32)> = params
                        .iter()
                        .map(|p| (p.id.to_string(), p.name.clone(), p.value))
                        .collect();
                    shared.set_plugin_params(t, ui_params);

                    plugins[t] = Some(proc);
                    crate::clap_gui::probe(&mut inst, t);
                    // Estado guardado en el proyecto (parámetros, preset interno…).
                    if let Some(bytes) = state.as_deref().and_then(lanzadera_core::b64::decode) {
                        if clap_host::load_state(&mut inst, &bytes) {
                            println!("[lanzadera] estado restaurado en pista {}", t + 1);
                        }
                    }
                    loaded.push((t, 0, 0, path.clone()));
                    instances.push((t, inst));
                }
                Err(e) => {
                    crate::clap_params::clear_track_params(t);
                    crate::clap_gui::notify(format!("⚠ CLAP pista {}: {}", t + 1, e));
                }
            }
        }
        (plugins, instances, Some(tx), Some(rx))
    };
    #[cfg(feature = "clap")]
    let _ = &mut plugins;

    #[cfg(feature = "vst3")]
    let mut vst3_plugins: [Option<vst3_slot::Vst3Slot>; MAX_TRACKS] = std::array::from_fn(|_| None);
    #[cfg(feature = "vst3")]
    let mut vst3_handles = Vec::new();
    #[cfg(feature = "vst3")]
    {
        let plugs: Vec<(usize, String, Option<String>)> = {
            let st = shared.state.lock().unwrap();
            st.tracks.iter().enumerate()
                .filter_map(|(i, tr)| tr.plugin_path.as_ref()
                    .filter(|p| p.trim_end_matches('/').ends_with(".vst3"))
                    .map(|p| (i, p.clone(), tr.plugin_state.clone())))
                .collect()
        };
        for (t, path, state) in plugs {
            match vst3_slot::load(&path, sample_rate, MAX_BLOCK as u32) {
                Ok(slot) => {
                    println!(
                        "[lanzadera] VST3 pista {}: {} · {} parámetros ({}) · editor={}",
                        t + 1,
                        slot.name,
                        slot.params.len(),
                        if slot.is_instrument { "instrumento" } else { "efecto" },
                        slot.has_editor
                    );
                    // Tabla RT param_idx -> id (misma que CLAP). VST3 trabaja en
                    // 0..1 normalizado, asi que min=0 y max=1: plain == normalizado.
                    let infos: Vec<clap_host::PluginParamInfo> = slot.params.iter()
                        .map(|p| clap_host::PluginParamInfo {
                            id: p.id, name: p.name.clone(), value: p.value, min: 0.0, max: 1.0,
                        }).collect();
                    crate::clap_params::set_track_params(t, &infos);
                    let ui_params: Vec<(String, String, f32)> = slot.params
                        .iter()
                        .map(|p| (p.id.to_string(), p.name.clone(), p.value))
                        .collect();
                    shared.set_plugin_params(t, ui_params);
                    crate::clap_gui::set_avail(t, slot.has_editor);
                    crate::clap_gui::set_open(t, false);
                    vst3_handles.push((t, slot.plugin.clone()));
                    if let Some(bytes) = state.as_deref().and_then(lanzadera_core::b64::decode) {
                        if let Ok(mut pl) = slot.plugin.lock() { let _ = pl.load_state(&bytes); }
                    }
                    loaded.push((t, 0, 0, path.clone()));
                    vst3_plugins[t] = Some(slot);
                }
                Err(e) => {
                    crate::clap_params::clear_track_params(t);
                    crate::clap_gui::notify(format!("⚠ VST3 pista {}: {}", t + 1, e));
                }
            }
        }
    }
    #[cfg(feature = "vst3")]
    let _ = &mut vst3_plugins;

    // ── Note FX / FX por pista (paso 2) ──
    // Note FX: solo CLAP y solo en pistas con instrumento plugin (las notas llegan al
    // instrumento por la cola neutral pev). FX: CLAP o VST3, en pistas MIDI y de audio.
    #[cfg(feature = "clap")]
    let mut note_fx: [Vec<Processor>; MAX_TRACKS] = std::array::from_fn(|_| Vec::new());
    #[allow(unused_mut)]
    let mut fx_chains: [Vec<FxInst>; MAX_TRACKS] = std::array::from_fn(|_| Vec::new());
    #[cfg(feature = "clap")]
    let mut fx_instances: Vec<(usize, clap_host::Instance)> = Vec::new();
    #[cfg(feature = "vst3")]
    let mut fx_vst3_handles: Vec<(usize, std::sync::Arc<Mutex<vst3_host::plugin::Plugin>>)> = Vec::new();
    {
        type Chain = Vec<lanzadera_core::track::PluginSlot>;
        let slots: Vec<(usize, Chain, Chain, bool)> = {
            let st = shared.state.lock().unwrap();
            st.tracks.iter().enumerate().take(MAX_TRACKS).map(|(i, tr)| (
                i,
                tr.note_fx_chain.clone(),
                tr.fx_chain.clone(),
                tr.plugin_path.is_some(),
            )).collect()
        };
        for (t, nchain, fchain, has_instr) in slots {
            if nchain.len() > crate::clap_gui::CHAIN_MAX || fchain.len() > crate::clap_gui::CHAIN_MAX {
                crate::clap_gui::notify(format!("⚠ Pista {}: máximo {} plugins por cadena; el resto se ignora", t + 1, crate::clap_gui::CHAIN_MAX));
            }
            if !has_instr && nchain.iter().any(|sl| !sl.bypass) {
                crate::clap_gui::notify(format!("⚠ Note FX pista {}: la pista no tiene instrumento plugin; no se aplica", t + 1));
            }
            for (_i, slot) in nchain.iter().enumerate().take(crate::clap_gui::CHAIN_MAX) {
                if slot.bypass || !has_instr { continue; }
                if slot.path.trim_end_matches('/').ends_with(".vst3") {
                    crate::clap_gui::notify(format!("⚠ Note FX pista {}: los Note FX VST3 aún no se procesan; usa la versión CLAP", t + 1));
                    continue;
                }
                #[cfg(feature = "clap")]
                {
                    let i = _i;
                    let gui_id = crate::clap_gui::nfx_gui_id(t, i);
                    match clap_host::load(&slot.path, slot.id.as_deref(), sample_rate, MAX_BLOCK as u32, gui_id) {
                        Ok((mut inst, proc, name, _params)) => {
                            println!("[lanzadera] Note FX {} pista {}: {}", i + 1, t + 1, name);
                            crate::clap_gui::probe(&mut inst, gui_id);
                            if let Some(bytes) = slot.state.as_deref().and_then(lanzadera_core::b64::decode) {
                                let _ = clap_host::load_state(&mut inst, &bytes);
                            }
                            loaded.push((t, 1, i, slot.path.clone()));
                            note_fx[t].push(proc);
                            fx_instances.push((gui_id, inst));
                        }
                        Err(e) => crate::clap_gui::notify(format!("⚠ Note FX {} pista {}: {}", i + 1, t + 1, e)),
                    }
                }
                #[cfg(not(feature = "clap"))]
                let _ = _i;
            }
            for (i, slot) in fchain.iter().enumerate().take(crate::clap_gui::CHAIN_MAX) {
                if slot.bypass { continue; }
                #[cfg(feature = "clap")]
                let gui_id = crate::clap_gui::fx_gui_id(t, i);
                #[cfg(not(feature = "clap"))]
                let _gui_id = crate::clap_gui::fx_gui_id(t, i);
                if slot.path.trim_end_matches('/').ends_with(".vst3") {
                    #[cfg(feature = "vst3")]
                    match vst3_slot::load(&slot.path, sample_rate, MAX_BLOCK as u32) {
                        Ok(vslot) => {
                            if vslot.is_instrument {
                                crate::clap_gui::notify(format!("⚠ FX pista {}: «{}» es un instrumento, no un efecto", t + 1, vslot.name));
                            } else {
                                println!("[lanzadera] FX VST3 {} pista {}: {}", i + 1, t + 1, vslot.name);
                                #[cfg(feature = "clap")]
                                {
                                    crate::clap_gui::set_avail(gui_id, vslot.has_editor);
                                    crate::clap_gui::set_open(gui_id, false);
                                }
                                if let Some(bytes) = slot.state.as_deref().and_then(lanzadera_core::b64::decode) {
                                    if let Ok(mut pl) = vslot.plugin.lock() { let _ = pl.load_state(&bytes); }
                                }
                                loaded.push((t, 2, i, slot.path.clone()));
                                #[cfg(feature = "clap")]
                                fx_vst3_handles.push((gui_id, vslot.plugin.clone()));
                                #[cfg(not(feature = "clap"))]
                                let _ = gui_id;
                                fx_chains[t].push(FxInst::Vst3(vslot));
                            }
                        }
                        Err(e) => crate::clap_gui::notify(format!("⚠ FX VST3 {} pista {}: {}", i + 1, t + 1, e)),
                    }
                } else {
                    #[cfg(feature = "clap")]
                    match clap_host::load(&slot.path, slot.id.as_deref(), sample_rate, MAX_BLOCK as u32, gui_id) {
                        Ok((mut inst, proc, name, _params)) => {
                            println!("[lanzadera] FX CLAP {} pista {}: {}", i + 1, t + 1, name);
                            crate::clap_gui::probe(&mut inst, gui_id);
                            if let Some(bytes) = slot.state.as_deref().and_then(lanzadera_core::b64::decode) {
                                let _ = clap_host::load_state(&mut inst, &bytes);
                            }
                            loaded.push((t, 2, i, slot.path.clone()));
                            fx_chains[t].push(FxInst::Clap(proc));
                            fx_instances.push((gui_id, inst));
                        }
                        Err(e) => crate::clap_gui::notify(format!("⚠ FX CLAP {} pista {}: {}", i + 1, t + 1, e)),
                    }
                }
            }
        }
    }

    // ── Motor de drums de Caja (Fase 4A) ──
    // El store vive en SessionShared: la UI lo necesita para saber qué
    // muestra sonó (cur_idx) y cuál está fijada (pin_idx).
    let caja_store = shared.caja_store.clone();
    let caja_loader = CajaLoader::spawn(caja_store.clone(), sample_rate);
    let caja_kit = default_kit_state().pads;
    let caja_host: [(u32, u8, u8); NUM_PADS] =
        std::array::from_fn(|i| (PadLive::default_for(i).seed, 0, ALL_LOCK));
    let caja_scheduler = CajaScheduler::new(shared.caja_sched.clone(), sample_rate as f64);

    // ── Input stream (Fase F2) ──
    let audio_in = if std::env::var_os("LANZADERA_NO_AUDIO_IN").is_some() {
        println!("[lanzadera] audio input: desactivado (LANZADERA_NO_AUDIO_IN=1)");
        (None, None, 0u16)
    } else {
        match crate::audio_input::open(&host, sample_rate as usize * 8) {
            Ok((stream, cons)) => {
                println!(
                    "[lanzadera] audio input: {} Hz · {} canales",
                    stream.sample_rate, stream.channels
                );
                let ch = stream.channels;
                (Some(stream), Some(cons), ch)
            }
            Err(e) => {
                eprintln!("[lanzadera] audio input: {}", e);
                (None, None, 0u16)
            }
        }
    };
    let (audio_in_stream, audio_in_cons, audio_in_ch) = audio_in;
    shared.input_channels.store(audio_in_ch.min(255) as u8, std::sync::atomic::Ordering::Relaxed);

    let mut renderer = Renderer {
        engine: ClipEngine::new(sample_rate as f64),
        synth: Synth::new(sample_rate as f32),
        eq: EqBank::new(),
        voices: std::array::from_fn(|_| TrackVoice { audio: None, midi: None }),
        events: Vec::with_capacity(256),
        midi_out: MidiOut::new(),
        gains: [(0.0, 0.0, false); MAX_TRACKS],
        peaks: [0.0; MAX_TRACKS + 1],
        groove_peak: 0.0,
        was_playing: false,
        rec_pending: [None; 32],
        sustain_down: [false; MAX_TRACKS],
        sustained: [[false; 128]; MAX_TRACKS],
        rec_on: false,
        rec_clip: None,
        rec_seq_seen: 0,
        rec_audio_slot: None,
        rec_origin_ticks: 0,
        last_global_ticks: 0,
        channels,
        sample_rate,
        bufs: Box::new([[[0.0; MAX_BLOCK]; 2]; MAX_TRACKS]),
        #[cfg(feature = "clap")]
        plugins,
        #[cfg(feature = "vst3")]
        vst3_plugins,
        #[cfg(feature = "clap")]
        note_fx,
        fx_chains,
        #[cfg(feature = "clap")]
        nfx_in: EventBuffer::with_capacity(MAX_EVENTS),
        #[cfg(feature = "clap")]
        nfx_out: EventBuffer::with_capacity(MAX_EVENTS),
        #[cfg(feature = "clap")]
        nfx_src: Box::new(crate::plugin_events::PluginEvents::new()),
        #[cfg(feature = "clap")]
        nfx_keep: Box::new(crate::plugin_events::PluginEvents::new()),
        #[cfg(feature = "clap")]
        fxin: Box::new([[0.0; MAX_BLOCK]; 2]),
        #[cfg(feature = "clap")]
        fx_ports: AudioPorts::with_capacity(2, 1),
        #[cfg(feature = "clap")]
        ev: std::array::from_fn(|_| EventBuffer::with_capacity(MAX_EVENTS)),
        #[cfg(feature = "clap")]
        pev: Box::new(std::array::from_fn(|_| crate::plugin_events::PluginEvents::new())),
        #[cfg(feature = "clap")]
        pbuf: Box::new([[0.0; MAX_BLOCK]; 2]),
        #[cfg(feature = "clap")]
        out_ports: AudioPorts::with_capacity(2, 1),
        #[cfg(feature = "clap")]
        steady: 0,
        #[cfg(feature = "clap")]
        retire: retire_tx,
        caja_engine: CajaEngine::new(sample_rate as f32),
        caja_store,
        caja_kit,
        caja_kit_live: [crate::groove_kit::PadMix::default(); NUM_PADS],
        caja_host,
        caja_buf: Box::new([[0.0; MAX_BLOCK]; 2]),
        caja_trigs: std::array::from_fn(|_| (0, 0, 0.0)),
        caja_trig_count: 0,
        caja_scheduler,
        groove_fx_filter_l: GmBiquad::default(),
        groove_fx_filter_r: GmBiquad::default(),
        groove_fx_delay_l: Box::new([0.0; 131072]),
        groove_fx_delay_r: Box::new([0.0; 131072]),
        groove_fx_delay_w: 0,
        groove_fx_tape_env: 1.0,
        audio_in_cons,
        audio_in_ch,
        audio_in_scratch: Vec::with_capacity(MAX_BLOCK * (audio_in_ch.max(2) as usize)),
        rec_mono: Vec::with_capacity(MAX_BLOCK),
        metro_env: 0.0,
        metro_freq: 1000.0,
        metro_phase: 0.0,
        record_engine: crate::audio_record::RecordEngine::new(sample_rate, 180),
        was_recording: false,
    };
    let _ = &mut renderer;

    let cb_shared = shared.clone();
    let stream = device.build_output_stream(
        &cfg,
        move |out: &mut [f32], _info| {
            // Paso 1 CLAP: marca este hilo como el de audio para
            // clap_host_thread_check. Idempotente y RT-safe: es un único
            // store en un thread-local (sin locks, sin alloc, ~1 ns).
            // Además hace que clap_services::wake() NO toque egui desde
            // aquí: un request_callback en RT solo setea el flag y lo
            // atiende el hilo principal en el siguiente poll().
            #[cfg(feature = "clap")]
            crate::clap_services::mark_audio_thread();

            let chunk = MAX_BLOCK * channels.max(1);
            for piece in out.chunks_mut(chunk) { renderer.render(piece, &cb_shared); }
        },
        |e| eprintln!("[lanzadera] error de audio: {}", e),
        None,
    ).map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;

    Ok(AudioHandle {
        stream: Some(stream),
        sample_rate,
        caja_loader: Some(caja_loader),
        #[cfg(feature = "clap")]
        instances,
        #[cfg(feature = "clap")]
        fx_instances,
        loaded,
        #[cfg(feature = "clap")]
        retire_rx,
        #[cfg(feature = "clap")]
        clap_embed: crate::clap_gui::EmbedHost::default(),
        #[cfg(feature = "vst3")]
        vst3_handles,
        #[cfg(feature = "vst3")]
        fx_vst3_handles,
        #[cfg(feature = "vst3")]
        vst3_gui: crate::vst3_slot::Vst3Gui::default(),
        in_stream: audio_in_stream,
    })
}

// ── Grabación cuantizada (RecQueued → Recording) ──────────────────────
// Latch de la puerta de captura. Hay una sola stream de audio por
// proceso, así que un estado global atómico basta y evita tocar el
// Renderer. Solo atomics: sin locks, sin reservas, sin I/O en el callback.
static REC_ARMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static REC_REQ_PREV: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static REC_Q_BASE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// ¿Toca capturar ya? Con `q_ticks == 0` (Immediate) captura al instante.
/// Si no, espera a entrar en un quantum nuevo del reloj global.
///
/// Se compara `now/q > base/q` en vez de `now % q == 0`: a 120 BPM el
/// reloj avanza ~3 ticks por bloque, así que un módulo exacto podría
/// saltarse la frontera y no abrir nunca la puerta.
#[inline]
fn rec_gate(recording: bool, playing: bool, q_ticks: u32, now_tick: u64) -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    if !recording || !playing {
        REC_ARMED.store(false, Relaxed);
        REC_REQ_PREV.store(false, Relaxed);
        return false;
    }
    if !REC_REQ_PREV.swap(true, Relaxed) {
        REC_Q_BASE.store(now_tick, Relaxed);
        if q_ticks == 0 {
            REC_ARMED.store(true, Relaxed);
        }
    }
    if !REC_ARMED.load(Relaxed) {
        let q = q_ticks as u64;
        let base = REC_Q_BASE.load(Relaxed);
        // Que el reloj vuelva atrás (una escena reinició el transporte en 1.1) también es una
        // frontera: sin esto la puerta no se abriría hasta recuperar el tick anterior.
        if q > 0 && (now_tick / q > base / q || now_tick < base) {
            REC_ARMED.store(true, Relaxed);
        }
    }
    REC_ARMED.load(Relaxed)
}
