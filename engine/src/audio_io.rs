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
    #[allow(dead_code)] // mantiene vivo el stream de entrada
    in_stream: Option<crate::audio_input::AudioInputStream>,
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
            }
        }
        // GUI CLAP embebidas: cierre por la X y resizes del plugin.
        crate::clap_gui::poll_embedded(&mut self.instances, &mut self.clap_embed);

        // GUI VST3: run loop del plugin + cierre por la X.
        #[cfg(feature = "vst3")]
        for t in self.vst3_gui.poll(&self.vst3_handles) {
            crate::clap_gui::set_open(t, false);
            println!("[gui] pista {}: el usuario cerro la ventana VST3", t + 1);
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

        let Some((_, inst)) = self.instances.iter_mut().find(|(i, _)| *i == t) else {
            crate::clap_gui::notify(format!("[gui] pista {} no tiene plugin cargado", t + 1));
            return;
        };
        match crate::clap_gui::toggle(inst, t, &title, &mut self.clap_embed) {
            Ok(()) => {
                let (_, open) = crate::clap_gui::state(t);
                println!("[gui] pista {}: {}", t + 1, if open { "abierta" } else { "cerrada" });
            }
            Err(e) => crate::clap_gui::notify(format!("[gui] pista {}: {}", t + 1, e)),
        }
    }
}

#[cfg(feature = "clap")]
impl Drop for AudioHandle {
    fn drop(&mut self) {
        // 0) Cerrar las GUI abiertas ANTES de soltar nada: la ventana del
        //    plugin debe destruirse mientras la instancia sigue viva.
        crate::clap_gui::close_all(&mut self.instances, &mut self.clap_embed);
        #[cfg(feature = "vst3")]
        {
            for (t, _) in self.vst3_handles.iter() { crate::clap_gui::set_open(*t, false); }
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
            std::thread::Builder::new()
                .name("vst3-teardown".into())
                .spawn(move || {
                    gui.close_all();  // editores primero (la vista antes que el plugin)
                    drop(handles);    // luego los plugins (teardown wine)
                })
                .ok();
        }
        // 2) Desactivar cada procesador con su instancia (hilo principal).
        if let Some(rx) = &self.retire_rx {
            while let Ok(p) = rx.recv_timeout(std::time::Duration::from_millis(300)) {
                if let Some((_, inst)) = self.instances.iter_mut().find(|(_, i)| p.matches(i)) {
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
    was_playing: bool,
    /// Notas en vivo pendientes de cerrar (grabación): (nota, vel, tick).
    rec_pending: [Option<(u8, u8, u32)>; 32],
    rec_on: bool,
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
    record_engine: crate::audio_record::RecordEngine,
    was_recording: bool,
    audio_in_cons: Option<ringbuf::HeapCons<f32>>,
    audio_in_ch: u16,
    audio_in_scratch: Vec<f32>,
    /// Envelope residual del click del metrónomo (decay por sample).
    metro_env: f32,
    metro_freq: f32,
    metro_phase: f32,
}

#[cfg(feature = "clap")]
impl Drop for Renderer {
    fn drop(&mut self) {
        if let Some(tx) = &self.retire {
            for slot in self.plugins.iter_mut() {
                if let Some(p) = slot.take() { let _ = tx.send(p.into_stopped()); }
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
        // Feedback del estado de grabación hacia la GUI y los LEDs.
        {
            let rt_t = audio.rec_track as usize;
            let rt_s = audio.rec_scene as usize;
            if audio.recording && !rec_live {
                shared.rt.set_clip_state(rt_t, rt_s, ClipState::RecQueued);
            } else if rec_live != self.rec_on {
                shared.rt.set_clip_state(
                    rt_t, rt_s,
                    if rec_live { ClipState::Recording } else { ClipState::Playing },
                );
            }
        }
        if self.rec_on && !rec_live {
            let t = audio.rec_track as usize;
            let now = self.rec_tick(audio, t);
            for slot in self.rec_pending.iter_mut() {
                if let Some((n, v, st)) = slot.take() {
                    let dur = if now >= st { now - st }
                              else { audio.rec_loop_ticks.saturating_sub(st) + now };
                    let _ = shared.rec_rx.push(RecNote { track: t as u8, note: n, vel: v, tick: st, dur: dur.max(1) });
                }
            }
        }
        let old_rec_on = self.rec_on;
        self.rec_on = rec_live;

        // Si acaba de abrirse la puerta de captura, detener de inmediato el
        // clip que estuviera sonando en esa pista. Sin esto, SessionState
        // pone el nuevo clip en Recording pero el motor sigue con la voz
        // anterior. Regla "una pista, una voz", inspirada en MAGDA (GPL-3).
        // Además fijamos el origen de ticks para que las notas entren en 0.1.
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
                            self.note(t, f as u32, MidiMsg::Off { note, ch: 0 });
                            if rec_live && t == audio.rec_track as usize {
                                let now = self.rec_tick(audio, t);
                                let found = self.rec_pending.iter_mut()
                                    .find(|s| matches!(s, Some((nn, _, _)) if *nn == note));
                                if let Some(slot) = found {
                                    if let Some((nn, v, st)) = slot.take() {
                                        let dur = if now >= st { now - st }
                                                  else { audio.rec_loop_ticks.saturating_sub(st) + now };
                                        let _ = shared.rec_rx.push(RecNote {
                                            track: t as u8, note: nn, vel: v, tick: st, dur: dur.max(1),
                                        });
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
                            let eff = loop_frames.min(df as f64);
                            let p = pos % eff;
                            let i0 = p as usize;
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
                let l = self.eq.bands[t].process(0, self.bufs[t][0][f]);
                let r = self.eq.bands[t].process(1, self.bufs[t][1][f]);
                ml += l * gl;
                mr += r * gr;
                let pk = l.abs().max(r.abs()) * gl.max(gr);
                if pk > self.peaks[t] { self.peaks[t] = pk; }
            }
            ml += self.caja_buf[0][f];
            mr += self.caja_buf[1][f];

            // Metrónomo: envelope + seno amortiguado en cada beat.
            if audio.metronome && audio.playing && ticks_per_sample > 0.0 {
                let prev = tick_f;
                tick_f += ticks_per_sample;
                let prev_beat = (prev / beat_ticks).floor();
                let cur_beat = (tick_f / beat_ticks).floor();
                if cur_beat > prev_beat {
                    let is_bar = (cur_beat as u64) % 4 == 0;
                    self.metro_env = if is_bar { 0.32 } else { 0.18 };
                    self.metro_freq = if is_bar { 2000.0 } else { 1000.0 };
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
                // Mezclar: primer canal de input → todos los canales de output.
                let out_ch = self.channels as usize;
                let gain = if audio.recording {
                    0.35
                } else if any_playing {
                    0.0
                } else {
                    0.8
                };
                for f in 0..frames {
                    let in_sample = self.audio_in_scratch[f * in_ch] * gain;
                    for c in 0..out_ch {
                        let idx = f * out_ch + c;
                        if idx < out.len() {
                            out[idx] += in_sample;
                        }
                    }
                }
                // Actualizar peaks del master con el input.
                for i in 0..needed {
                    let a = self.audio_in_scratch[i].abs();
                    if a > self.peaks[MAX_TRACKS] {
                        self.peaks[MAX_TRACKS] = a;
                    }
                }
            }
        }

        // ── RecordEngine (Fase F3) ──
        // Detectar cambios en audio.recording y arrancar/parar slots.
        if self.rec_on && !self.was_recording {
            let rt = audio.rec_track as usize;
            let rs = audio.rec_scene as usize;
            self.record_engine
                .start_recording(rt, rs, self.audio_in_ch, self.sample_rate);
            println!("[record] inicio: pista {} escena {}", rt + 1, rs + 1);
        } else if !self.rec_on && self.was_recording {
            let rt = audio.rec_track as usize;
            let rs = audio.rec_scene as usize;
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
            let in_valid = (frames * self.audio_in_ch as usize).min(self.audio_in_scratch.len());
            if in_valid > 0 {
                self.record_engine.write(&self.audio_in_scratch[..in_valid]);
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

    // Estado de GUI limpio en cada (re)arranque: si se quito un plugin, su
    // boton "Interfaz" no debe quedar activo.
    #[cfg(feature = "clap")]
    for t in 0..MAX_TRACKS {
        crate::clap_gui::set_avail(t, false);
        crate::clap_gui::set_open(t, false);
    }

    #[cfg(feature = "clap")]
    let (mut plugins, instances, retire_tx, retire_rx) = {
        let mut plugins: [Option<Processor>; MAX_TRACKS] = std::array::from_fn(|_| None);
        let mut instances = Vec::new();
        let (tx, rx) = std::sync::mpsc::channel();
        let plugs: Vec<(usize, String, Option<String>)> = {
            let st = shared.state.lock().unwrap();
            st.tracks.iter().enumerate()
                .filter_map(|(i, tr)| tr.plugin_path.clone().map(|p| (i, p, tr.plugin_id.clone())))
                .collect()
        };
        for (t, path, id) in plugs {
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
        let plugs: Vec<(usize, String)> = {
            let st = shared.state.lock().unwrap();
            st.tracks.iter().enumerate()
                .filter_map(|(i, tr)| tr.plugin_path.as_ref()
                    .filter(|p| p.trim_end_matches('/').ends_with(".vst3"))
                    .map(|p| (i, p.clone())))
                .collect()
        };
        for (t, path) in plugs {
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

    let mut renderer = Renderer {
        engine: ClipEngine::new(sample_rate as f64),
        synth: Synth::new(sample_rate as f32),
        eq: EqBank::new(),
        voices: std::array::from_fn(|_| TrackVoice { audio: None, midi: None }),
        events: Vec::with_capacity(256),
        midi_out: MidiOut::new(),
        gains: [(0.0, 0.0, false); MAX_TRACKS],
        peaks: [0.0; MAX_TRACKS + 1],
        was_playing: false,
        rec_pending: [None; 32],
        rec_on: false,
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
        audio_in_cons,
        audio_in_ch,
        audio_in_scratch: Vec::with_capacity(MAX_BLOCK * (audio_in_ch.max(2) as usize)),
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
        retire_rx,
        #[cfg(feature = "clap")]
        clap_embed: crate::clap_gui::EmbedHost::default(),
        #[cfg(feature = "vst3")]
        vst3_handles,
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
        if q > 0 && now_tick / q > REC_Q_BASE.load(Relaxed) / q {
            REC_ARMED.store(true, Relaxed);
        }
    }
    REC_ARMED.load(Relaxed)
}
