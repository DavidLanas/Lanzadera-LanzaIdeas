// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! SessionShared — equivalente a KitShared de Caja.
//! Fase 3: además de muestras, almacén de notas MIDI por slot (mismo patrón
//! ArcSwapOption: el audio lee con load_full, lock-free y sin alloc).
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering, AtomicU8, AtomicU64, AtomicU32, AtomicI8, AtomicUsize};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use lanzadera_core::audio_state::SessionAudio;
use lanzadera_core::control::ControlCmd;
use lanzadera_core::{Clip, ClipState, SessionState, Spsc, TripleBuffer, MAX_SCENES, MAX_TRACKS};
use crate::midi::MidiNoteData;
use crate::rec::RecRing;
use crate::caja_drums::{CajaSchedulerShared, CajaStore};
use caja_core::kit::{ALL_LOCK, ALL_ROLL, NUM_PADS, P_SAMPLE};
use crate::groove::GrooveModel;
use crate::groove_kit::GrooveKit;
use crate::rt::RtStore;

/// Muestras decodificadas por el loader (interleaved, al rate del stream).
pub struct SampleData {
    pub sample_rate: u32,
    pub channels: u8,
    pub data: Vec<f32>,
    pub peaks: Vec<f32>,
}

/// Almacén por slot (track × escena) con publicación atómica.
pub struct SlotStore<T> { slots: [ArcSwapOption<T>; MAX_TRACKS * MAX_SCENES] }

impl<T> SlotStore<T> {
    pub fn new() -> Self { SlotStore { slots: std::array::from_fn(|_| ArcSwapOption::from(None)) } }
    pub fn load(&self, t: usize, s: usize) -> Option<Arc<T>> { self.slots[t * MAX_SCENES + s].load_full() }
    pub fn store(&self, t: usize, s: usize, v: Arc<T>) { self.slots[t * MAX_SCENES + s].store(Some(v)); }
    pub fn clear(&self, t: usize, s: usize) { self.slots[t * MAX_SCENES + s].store(None); }
}

pub type SampleStore = SlotStore<SampleData>;
pub type NoteStore = SlotStore<MidiNoteData>;

pub enum LoadStatus {
    Idle,
    Loading { track: u8, scene: u8 },
    Ready,
    Error(String),
}

/// Petición de carga: lleva String → Mutex<VecDeque> (solo UI y loader).
pub struct LoadRequest { pub track: usize, pub scene: usize, pub path: String }

pub struct SessionShared {
    pub state: Mutex<SessionState>,
    pub audio: TripleBuffer<SessionAudio>,
    pub samples: SampleStore,
    pub notes: NoteStore,
    pub status: Mutex<LoadStatus>,
    pub persist: Arc<Mutex<String>>,
    pub resync: AtomicBool,
    pub rescan: AtomicBool,
    pub ctrl_rx: Arc<Spsc>,
    pub trig_rx: Spsc,
    /// Notas en vivo (teclado) UI → audio: cola propia, independiente
    /// del transporte y de los triggers de clip.
    pub live_rx: Spsc,
    /// Notas grabadas: audio → UI (anillo POD lock-free).
    pub rec_rx: RecRing,
    pub rt: Arc<RtStore>,
    pub load_queue: Arc<Mutex<VecDeque<LoadRequest>>>,
    pub loader_stop: AtomicBool,
    /// La UI pide reiniciar el stream (cambio de plugins CLAP).
    pub audio_restart: AtomicBool,
    /// Motor de patrones de Caja: calendario + step publicado.
    pub caja_sched: Arc<CajaSchedulerShared>,
    /// Almacén de muestras de Groove Machine. Compartido con el loader y la
    /// UI: `cur_idx` dice qué muestra sonó y `pin_idx` cuál está fijada.
    pub caja_store: Arc<CajaStore>,
    /// Groove Machine visible → MK3 cambia de comportamiento
    pub groove_machine_visible: AtomicBool,
    /// Canales del dispositivo de entrada abierto (0 = sin entrada). Lo publica el audio.
    pub input_channels: AtomicU8,
    /// 0=Pad 1=Step 2=Mixer 3=Browser (sincronizado desde UI)
    pub groove_sub_mode: AtomicU8,
    /// Vista del groove persistida en la sesión: 0=Piano Roll, 1=Rack.
    pub groove_view: AtomicU8,
    /// Compás visible persistido en la sesión.
    pub groove_bar: AtomicUsize,
    /// Página visible persistida en la sesión (Piano Roll).
    pub groove_page: AtomicUsize,
    /// Estado de seguimiento del playhead para el Groove Machine.
    pub groove_follow_playhead: AtomicBool,
    /// Bitmask de botones del MK3 físicamente pulsados (bit = cc - 20).
    /// Lo escribe el controller thread; lo lee el LED thread para el flash.
    pub mk3_buttons: AtomicU64,
    /// Flags varios del MK3: bit0=fixed_vel bit1=vol_encoder bit2=swing bit3=tempo_enc
    pub mk3_flags: AtomicU8,
    /// Petición de cambio de pestaña desde el hardware: 255=ninguna, 0=Pad 1=Step 2=Mixer 3=Samples
    pub mk3_mode_req: AtomicU8,
    /// Visibilidad de ventanas: bit0=clip bit1=groove bit2=inspector bit3=browser bit4=mixer
    pub window_flags: AtomicU8,
    /// Petición de toggle de ventana desde el hardware: 255=ninguna,
    /// 0=ClipLaunch 1=GrooveMachine 2=Inspector 3=Browser 4=Mixer
    pub mk3_win_req: AtomicU8,
    /// Celda/pad de Groove Machine seleccionado (0..24), compartido UI ↔ MK3.
    pub groove_sel_pad: AtomicU8,
    /// Modo del encoder tras SWING: 0=NAV 1=Shuffle 2=Accent 3=Human 4=Density
    pub mk3_swing_mode: AtomicU8,
    /// Petición de compás: 255=ninguna 0=anterior 1=siguiente
    pub mk3_bar_req: AtomicU8,
    /// Frase para generar patrón (ERASE). La escribe la UI, la lee el MK3.
    pub groove_phrase: Arc<Mutex<String>>,
    /// Modo EVENT: los 16 pads son los pasos de la celda seleccionada.
    pub mk3_event_mode: AtomicBool,
    /// Modo Seguir/Editar de la vista Groove, reflejado en el LED EVENTS.
    pub mk3_follow_playhead: AtomicBool,
    /// Modo PATTERN: los 16 pads son slots del rack.
    pub mk3_pattern_mode: AtomicBool,
    /// CHORDS activo. Capa estable e independiente del sub-modo de pestaña:
    /// la escribe solo la UI, una vez por frame, para que los LEDs no pestañeen.
    pub mk3_chords_mode: AtomicBool,
    /// Destino del ENCODER en CHORDS: 255 = navegar celdas,
    /// 1 = Pitch, 2 = Decay, 3 = Filtro. Se asigna aunque el parámetro esté
    /// bloqueado: el lock detiene el dado, no la edición manual.
    pub mk3_chords_param: AtomicU8,
    /// FX PAD seleccionado en MK3: 0=PITCH, 1=MOD, 2=PERFORM.
    pub mk3_pad_fx: AtomicU8,
    /// Modo temporal de presets rápidos del MK3; no cambia el modo PATTERN.
    pub mk3_preset_mode: AtomicBool,
    /// Bits 0..7: cargas de slots rápidos pendientes desde el MK3.
    pub mk3_preset_load_req: AtomicU8,
    /// Bits 0..7: guardados de slots rápidos pendientes desde el MK3.
    pub mk3_preset_save_req: AtomicU8,
    /// Paso pedido en modo EVENT: 255=ninguno, 0..15=pad.
    pub mk3_step_req: AtomicU8,
    /// Pieza (no pad) seleccionada, publicada por la UI para los LEDs.
    pub groove_sel_piece: AtomicU8,
    /// Bitmask de slots del rack con contenido (32 bits).
    pub groove_rack_bits: AtomicU32,
    /// Slot activo del rack (0..31).
    pub groove_active_pat: AtomicU8,
    /// Pad mantenido en modo EVENT (255 = ninguno).
    pub mk3_held_pad: AtomicU8,
    /// Delta de velocidad pedido por pad+ENCODER (0 = nada).
    pub mk3_vel_req: AtomicI8,
    /// Pasos de la celda elegida en el compás visible: bits 0-15 on, 16-31 ghost.
    pub gm_step_bits: AtomicU32,
    /// Pasos EVENT con velocidad reducida y no ghost (bits 0..15).
    pub gm_step_soft_bits: AtomicU32,
    /// Piezas que suenan en el paso actual (bits 0-24) → ritmo en modo PAD.
    pub gm_hit_bits: AtomicU32,
    /// Paso del playhead dentro del compás (0-15, 255 = parado).
    pub gm_playhead: AtomicU8,
    /// Motor completo de Groove Machine (Session de caja-core).
    pub groove: Arc<GrooveModel>,
    pub kit: Arc<GrooveKit>,
}

/// JSON del patrón de Groove tal como queda al arrancar (para «Nuevo proyecto»).
static GROOVE_DEFAULT_JSON: std::sync::OnceLock<String> = std::sync::OnceLock::new();

impl SessionShared {
    pub fn new() -> Arc<Self> {
        let state = SessionState::default();
        let audio = TripleBuffer::new(|| SessionAudio::from_state(&state));
        let persist = Arc::new(Mutex::new(state.to_json()));
        let groove_kit = GrooveKit::new();
        let groove_sched = Arc::new(CajaSchedulerShared::default());
        let groove_model = Arc::new(GrooveModel::new(groove_sched.clone()));
        // Patrón inicial del Groove: lo usa «Archivo → Nuevo» para volver a empezar limpio.
        let _ = GROOVE_DEFAULT_JSON.set(groove_model.to_json());
        Arc::new(SessionShared {
            state: Mutex::new(state), audio,
            samples: SampleStore::new(), notes: NoteStore::new(),
            status: Mutex::new(LoadStatus::Idle), persist,
            resync: AtomicBool::new(false), rescan: AtomicBool::new(false),
            ctrl_rx: Arc::new(Spsc::default()), trig_rx: Spsc::default(),
            live_rx: Spsc::default(),
            rec_rx: RecRing::new(),
            rt: Arc::new(RtStore::new()),
            load_queue: Arc::new(Mutex::new(VecDeque::new())),
            loader_stop: AtomicBool::new(false),
            audio_restart: AtomicBool::new(false),
            caja_sched: groove_sched.clone(),
            caja_store: Arc::new(CajaStore::new()),
            groove: groove_model,
            kit: groove_kit,
            groove_machine_visible: AtomicBool::new(false),
            input_channels: AtomicU8::new(0),
            groove_sub_mode: AtomicU8::new(0),
            groove_view: AtomicU8::new(0),
            groove_bar: AtomicUsize::new(0),
            groove_page: AtomicUsize::new(0),
            groove_follow_playhead: AtomicBool::new(true),
            mk3_buttons: AtomicU64::new(0),
            mk3_flags: AtomicU8::new(0),
            mk3_mode_req: AtomicU8::new(255),
            window_flags: AtomicU8::new(0b00101),
            mk3_win_req: AtomicU8::new(255),
            groove_sel_pad: AtomicU8::new(0),
            mk3_swing_mode: AtomicU8::new(0),
            mk3_bar_req: AtomicU8::new(255),
            groove_phrase: Arc::new(Mutex::new(String::new())),
            mk3_event_mode: AtomicBool::new(false),
            mk3_follow_playhead: AtomicBool::new(true),
            mk3_pattern_mode: AtomicBool::new(false),
            mk3_chords_mode: AtomicBool::new(false),
            mk3_chords_param: AtomicU8::new(255),
            mk3_pad_fx: AtomicU8::new(0),
            mk3_preset_mode: AtomicBool::new(false),
            mk3_preset_load_req: AtomicU8::new(0),
            mk3_preset_save_req: AtomicU8::new(0),
            mk3_step_req: AtomicU8::new(255),
            groove_sel_piece: AtomicU8::new(0),
            groove_rack_bits: AtomicU32::new(0),
            groove_active_pat: AtomicU8::new(0),
            mk3_held_pad: AtomicU8::new(255),
            mk3_vel_req: AtomicI8::new(0),
            gm_step_bits: AtomicU32::new(0),
            gm_step_soft_bits: AtomicU32::new(0),
            gm_hit_bits: AtomicU32::new(0),
            gm_playhead: AtomicU8::new(255),
        })
    }

    pub fn edit<R>(&self, f: impl FnOnce(&mut SessionState) -> R) -> R {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let r = f(&mut s);
        let mut snapshot = SessionAudio::from_state(&s);
        // Alinear quantize del launcher al ciclo de Groove Machine cuando está activo.
        if self.caja_sched.enabled.load(Ordering::Relaxed) {
            if let Ok(sch) = self.caja_sched.schedule.lock() {
                snapshot.groove_loop_ticks = sch.loop_ticks;
            }
        } else {
            snapshot.groove_loop_ticks = 0;
        }
        self.audio.publish(|a| *a = snapshot);
        if let Ok(mut p) = self.persist.lock() { *p = s.to_json(); }
        self.resync.store(true, Ordering::Relaxed);
        r
    }

    /// Encola una petición de carga de slot desde el controlador.
    pub fn request_mk3_preset_load(&self, slot: usize) {
        if slot < 8 {
            self.mk3_preset_load_req.fetch_or(
                1u8 << slot,
                Ordering::Relaxed,
            );
        }
    }

    /// Encola una petición de guardado de slot desde el controlador.
    pub fn request_mk3_preset_save(&self, slot: usize) {
        if slot < 8 {
            self.mk3_preset_save_req.fetch_or(
                1u8 << slot,
                Ordering::Relaxed,
            );
        }
    }

    /// Consume las peticiones pendientes. Devuelve (cargas, guardados).
    pub fn take_mk3_preset_requests(&self) -> (u8, u8) {
        let loads = self.mk3_preset_load_req.swap(0, Ordering::Relaxed);
        let saves = self.mk3_preset_save_req.swap(0, Ordering::Relaxed);
        (loads, saves)
    }

    /// Publica las notas MIDI de un slot al hilo de audio (ordenadas por tick).
    /// La llama la UI tras editar el PianoRoll, crear un clip o cargar proyecto.
    pub fn sync_midi(&self, t: usize, s: usize) {
        let notes = {
            let st = self.state.lock().unwrap();
            st.tracks.get(t).and_then(|tr| tr.clips.get(s)).and_then(|c| c.as_ref())
                .map(|c| c.midi_notes.clone()).unwrap_or_default()
        };
        if notes.is_empty() { self.notes.clear(t, s); return; }
        let mut sorted = notes;
        sorted.sort_by_key(|n| n.tick_start);
        self.notes.store(t, s, Arc::new(MidiNoteData { notes: sorted }));
    }

    pub fn apply_control(&self, cmd: ControlCmd) {
        let resolved = self.edit(|s| { let r = s.resolve_cmd(cmd); s.apply_control(r); r });
        if matches!(resolved,
            ControlCmd::LaunchClip { .. } | ControlCmd::StopClip { .. } |
            ControlCmd::LaunchScene { .. } | ControlCmd::StopAll)
        {
            let _ = self.trig_rx.push(resolved.encode());
        }
        if matches!(resolved, ControlCmd::LiveNoteOn { .. } | ControlCmd::LiveNoteOff { .. } |
            ControlCmd::LivePitchBend { .. } | ControlCmd::LiveModWheel { .. } |
            ControlCmd::LiveSustain { .. } |
            ControlCmd::LivePadOn { .. } | ControlCmd::LivePadOff { .. }) {
            let _ = self.live_rx.push(resolved.encode());
        }
    }

    pub fn request_load(&self, t: usize, s: usize, path: String) {
        let name = std::path::Path::new(&path)
            .file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
        self.edit(|st| {
            if let Some(tr) = st.tracks.get_mut(t) {
                if let Some(slot) = tr.clips.get_mut(s) {
                    let c = slot.get_or_insert_with(Clip::default);
                    c.audio_path = Some(path.clone());
                    if c.name.is_empty() { c.name = name.clone(); }
                }
            }
        });
        *self.status.lock().unwrap() = LoadStatus::Loading { track: t as u8, scene: s as u8 };
        self.load_queue.lock().unwrap().push_back(LoadRequest { track: t, scene: s, path });
    }

    /// Asigna (o quita con None) el plugin CLAP de una pista y pide reinicio.
    /// Setea el valor de un parámetro del plugin (0-1) y pide reinicio
    /// para que el audio aplique el cambio (los plugins CLAP requieren
    /// que los parámetros se escriban antes de activate, o vía evento
    /// en el callback de process — esto último se implementa en 3H).
    pub fn set_plugin_param(&self, t: usize, idx: usize, value: f32) {
        self.edit(|st| {
            if let Some(tr) = st.tracks.get_mut(t) {
                if let Some((_, _, v)) = tr.plugin_params.get_mut(idx) {
                    *v = value.clamp(0.0, 1.0);
                }
            }
        });
    }

    /// Añade un plugin a la cadena Note FX (o la vacía con path=None). Reinicia el audio.
    pub fn set_note_fx(&self, t: usize, path: Option<String>, id: Option<String>) {
        self.edit(|st| {
            if let Some(tr) = st.tracks.get_mut(t) {
                match path {
                    Some(p) => tr.note_fx_chain.push(lanzadera_core::track::PluginSlot {
                        path: p, id, state: None, bypass: false,
                    }),
                    None => tr.note_fx_chain.clear(),
                }
            }
        });
        self.audio_restart.store(true, Ordering::Relaxed);
    }
    /// Quita el slot i de la cadena Note FX. Reinicia el audio.
    pub fn remove_note_fx(&self, t: usize, idx: usize) {
        self.edit(|st| {
            if let Some(tr) = st.tracks.get_mut(t) {
                if idx < tr.note_fx_chain.len() { tr.note_fx_chain.remove(idx); }
            }
        });
        self.audio_restart.store(true, Ordering::Relaxed);
    }

    /// Añade un plugin a la cadena FX (o la vacía con path=None). Reinicia el audio.
    pub fn set_fx(&self, t: usize, path: Option<String>, id: Option<String>) {
        self.edit(|st| {
            if let Some(tr) = st.tracks.get_mut(t) {
                match path {
                    Some(p) => tr.fx_chain.push(lanzadera_core::track::PluginSlot {
                        path: p, id, state: None, bypass: false,
                    }),
                    None => tr.fx_chain.clear(),
                }
            }
        });
        self.audio_restart.store(true, Ordering::Relaxed);
    }
    /// Quita el slot i de la cadena FX. Reinicia el audio.
    pub fn remove_fx(&self, t: usize, idx: usize) {
        self.edit(|st| {
            if let Some(tr) = st.tracks.get_mut(t) {
                if idx < tr.fx_chain.len() { tr.fx_chain.remove(idx); }
            }
        });
        self.audio_restart.store(true, Ordering::Relaxed);
    }

    pub fn set_plugin(&self, t: usize, path: Option<String>, id: Option<String>) {
        let had_path = path.is_some();
        self.edit(|st| {
            if let Some(tr) = st.tracks.get_mut(t) {
                tr.plugin_path = path.clone();
                tr.plugin_id = id.clone();
                if !had_path {
                    tr.plugin_params.clear();
                }
            }
        });
        if !had_path {
            #[cfg(feature = "clap")]
            crate::clap_params::clear_track_params(t);
        }
        self.audio_restart.store(true, Ordering::Relaxed);
    }

    // ── Presets de Groove Machine ─────────────────────────────────────

    /// Lista los nombres de presets guardados en la sesión.
    pub fn list_groove_presets(&self) -> Vec<String> {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.groove_presets.iter().map(|p| p.name.clone()).collect()
    }

    /// Guarda el Groove actual como preset. Mismo nombre (sin distinguir
    /// mayúsculas) = reemplazar, no duplicar.
    pub fn save_groove_preset(&self, name: &str) -> Result<(), String> {
        let cleaned = name.trim();
        if cleaned.is_empty() {
            return Err("El preset necesita un nombre.".to_string());
        }
        let preset = lanzadera_core::session::GroovePreset {
            name: cleaned.to_string(),
            json: self.groove.to_json(),
        };
        self.edit(|st| {
            if let Some(pos) = st.groove_presets.iter()
                .position(|p| p.name.eq_ignore_ascii_case(cleaned))
            {
                st.groove_presets[pos] = preset;
            } else {
                st.groove_presets.push(preset);
            }
        });
        Ok(())
    }

    /// Carga un preset por nombre en el GrooveModel (no toca la vista).
    pub fn load_groove_preset(&self, name: &str) -> Result<(), String> {
        let target = name.trim();
        let json = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.groove_presets.iter()
                .find(|p| p.name.eq_ignore_ascii_case(target))
                .map(|p| p.json.clone())
                .ok_or_else(|| format!("Preset no encontrado: {}", target))?
        };
        self.groove.load_json(&json)
    }

    /// Borra un preset por nombre.
    pub fn delete_groove_preset(&self, name: &str) -> Result<(), String> {
        let target = name.trim();
        let mut found = false;
        self.edit(|st| {
            let before = st.groove_presets.len();
            st.groove_presets.retain(|p| !p.name.eq_ignore_ascii_case(target));
            found = st.groove_presets.len() != before;
        });
        if found { Ok(()) } else { Err(format!("Preset no encontrado: {}", target)) }
    }

    /// Guarda el Groove actual en uno de los ocho slots rápidos del MK3.
    /// El preset queda guardado en la lista de presets del proyecto.
    pub fn save_groove_preset_slot(&self, slot: usize) -> Result<(), String> {
        let slot = slot.min(7);
        let existing_name = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.groove_preset_slots[slot].clone()
        };
        let name = existing_name.unwrap_or_else(|| format!("MK3 Slot {}", slot + 1));

        self.save_groove_preset(&name)?;
        self.edit(|state| {
            state.groove_preset_slots[slot] = Some(name);
            state.groove_preset_active = slot;
        });
        Ok(())
    }

    /// Borra la referencia de un preset del slot, sin eliminarlo del banco global.
    pub fn clear_groove_preset_slot(&self, slot: usize) -> Result<(), String> {
        let slot = slot.min(7);
        {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.groove_preset_slots[slot].is_none() {
                return Err(format!("El slot {} ya está vacío.", slot + 1));
            }
        }

        self.edit(|state| {
            state.groove_preset_slots[slot] = None;
            if state.groove_preset_active == slot {
                state.groove_preset_active = state
                    .groove_preset_slots
                    .iter()
                    .position(|slot_name| slot_name.is_some())
                    .unwrap_or(0);
            }
        });
        Ok(())
    }

    /// Carga el preset asignado a un slot rápido del MK3.
    pub fn load_groove_preset_slot(&self, slot: usize) -> Result<(), String> {
        let slot = slot.min(7);
        let name = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.groove_preset_slots[slot]
                .clone()
                .ok_or_else(|| format!("Slot de preset {} vacío.", slot + 1))?
        };

        self.load_groove_preset(&name)?;
        self.edit(|state| state.groove_preset_active = slot);
        Ok(())
    }

    /// Selecciona el slot rápido que se muestra como activo en la UI.
    pub fn select_groove_preset_slot(&self, slot: usize) {
        let slot = slot.min(7);
        self.edit(|state| state.groove_preset_active = slot);
    }

    /// Guarda el Groove actual con el nombre indicado y lo asigna al slot.
    pub fn save_groove_preset_slot_named(
        &self,
        slot: usize,
        name: &str,
    ) -> Result<(), String> {
        let slot = slot.min(7);
        let name = name.trim();
        if name.is_empty() {
            return Err("Escribe un nombre para el preset.".to_string());
        }

        self.save_groove_preset(name)?;
        let name = name.to_string();
        self.edit(|state| {
            state.groove_preset_slots[slot] = Some(name);
            state.groove_preset_active = slot;
        });
        Ok(())
    }

    /// Exporta el Groove actual a un archivo .groovepreset (JSON).
    pub fn export_groove_preset(&self, path: &str) -> Result<(), String> {
        std::fs::write(path, self.groove.to_json()).map_err(|e| e.to_string())
    }

    /// Exporta el banco completo de presets y slots del Groove a un archivo JSON.
    /// Usa el `groove_bank_name` actual o un nombre por defecto si está vacío.
    #[allow(dead_code)] // reservado para el export de bancos
    fn default_bank_name(&self) -> String {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let name = state.groove_bank_name.trim();
        if name.is_empty() { "groove-bank".to_string() } else { name.to_string() }
    }

    pub fn export_groove_bank(&self, path: &str) -> Result<(), String> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let bank = serde_json::json!({
            "bank_name": state.groove_bank_name,
            "presets": state.groove_presets,
            "slots": state.groove_preset_slots,
            "active_slot": state.groove_preset_active
        });
        std::fs::write(path, bank.to_string()).map_err(|e| e.to_string())
    }

    /// Lee el banco y devuelve los nombres de presets, sin tocar el estado actual.
    /// Alterna una ruta en favoritos y devuelve el nuevo estado.
    pub fn toggle_groove_bank_favorite(&self, path: &str) -> bool {
        let path = path.trim().to_string();
        if path.is_empty() {
            return false;
        }

        let mut is_favorite = false;
        self.edit(|state| {
            if let Some(index) = state.groove_bank_favorites.iter().position(|p| p == &path) {
                state.groove_bank_favorites.remove(index);
            } else {
                state.groove_bank_favorites.push(path);
                is_favorite = true;
            }
        });
        is_favorite
    }

    /// Devuelve las rutas favoritas de bancos Groove.
    pub fn list_groove_bank_favorites(&self) -> Vec<String> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.groove_bank_favorites.clone()
    }

    /// Devuelve los bancos importados recientemente, más reciente primero.
    pub fn list_recent_groove_banks(&self) -> Vec<String> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.groove_bank_recent.clone()
    }

    /// Registra un banco importado, sin duplicados y con un máximo de ocho.
    pub fn push_recent_groove_bank(&self, path: &str) {
        let path = path.trim().to_string();
        if path.is_empty() {
            return;
        }

        self.edit(|state| {
            state.groove_bank_recent.retain(|entry| entry != &path);
            state.groove_bank_recent.insert(0, path);
            state.groove_bank_recent.truncate(8);
        });
    }

    pub fn preview_groove_bank(&self, path: &str) -> Result<Vec<String>, String> {
        let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let bank: serde_json::Value = serde_json::from_str(&content).map_err(|e| e.to_string())?;

        let presets = serde_json::from_value::<Vec<lanzadera_core::session::GroovePreset>>(
            bank.get("presets").cloned().ok_or_else(|| "El banco no contiene la lista de presets.".to_string())?
        ).map_err(|e| format!("Lista de presets inválida: {}", e))?;

        Ok(presets.into_iter().map(|p| p.name).collect())
    }

    /// Importa un banco completo de presets y slots del Groove.
    pub fn import_groove_bank(&self, path: &str) -> Result<(), String> {
        let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let bank: serde_json::Value = serde_json::from_str(&content).map_err(|e| e.to_string())?;

        let presets = serde_json::from_value::<Vec<lanzadera_core::session::GroovePreset>>(
            bank.get("presets")
                .cloned()
                .ok_or_else(|| "El banco no contiene la lista de presets.".to_string())?
        ).map_err(|e| format!("Lista de presets inválida: {}", e))?;

        let slots = serde_json::from_value::<[Option<String>; 8]>(
            bank.get("slots")
                .cloned()
                .ok_or_else(|| "El banco no contiene los ocho slots.".to_string())?
        ).map_err(|e| format!("Slots de preset inválidos: {}", e))?;

        let active = bank.get("active_slot")
            .and_then(|v| v.as_u64())
            .filter(|&slot| slot < 8)
            .ok_or_else(|| "El slot activo debe estar entre 0 y 7.".to_string())?
            as usize;

        self.edit(|state| {
            state.groove_presets = presets;
            state.groove_preset_slots = slots;
            state.groove_preset_active = active.min(7);
            if let Some(name) = bank.get("bank_name").and_then(|v| v.as_str()) {
                state.groove_bank_name = name.to_string();
            } else {
                state.groove_bank_name = "groove-bank".to_string();
            }
        });

        self.push_recent_groove_bank(path);
        Ok(())
    }

    /// Importa un .groovepreset: valida el JSON antes de tocar el estado
    /// y lo guarda también como preset de la sesión.
    pub fn import_groove_preset(&self, path: &str, name: &str) -> Result<(), String> {
        let json = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        self.groove.load_json(&json)?;
        let cleaned = name.trim();
        let final_name = if cleaned.is_empty() {
            std::path::Path::new(path)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Importado".to_string())
        } else {
            cleaned.to_string()
        };
        self.save_groove_preset(&final_name)
    }

    /// Guarda el proyecto: escritura atómica (tmp + rename), copia `.bak` del archivo
    /// anterior y el audio referenciado copiado a `<proyecto>_audio/` con rutas relativas
    /// (así el proyecto no depende de /tmp ni de dónde se grabó cada WAV).
    pub fn save_to_file(&self, path: &str) -> Result<(), String> { self.save_impl(path, true) }

    /// Autoguardado: mismo JSON, pero sin copiar audio, sin rotar `.bak` y sin tocar el
    /// estado del proyecto real.
    pub fn autosave_to_file(&self, path: &str) -> Result<(), String> { self.save_impl(path, false) }

    fn save_impl(&self, path: &str, final_save: bool) -> Result<(), String> {
        let proj = std::path::Path::new(path);
        let dir = proj.parent().filter(|d| !d.as_os_str().is_empty())
            .map(|d| d.to_path_buf()).unwrap_or_else(|| std::path::PathBuf::from("."));
        let stem = proj.file_stem().and_then(|x| x.to_str()).unwrap_or("proyecto").to_string();
        let audio_dir_name = format!("{}_audio", stem);
        let audio_dir = dir.join(&audio_dir_name);

        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());

        // El patrón Groove se guarda dentro del mismo proyecto que la sesión.
        s.groove_json = self.groove.to_json();
        s.groove_view = self.groove_view.load(Ordering::Relaxed).min(1);
        s.groove_bar = self.groove_bar.load(Ordering::Relaxed);
        s.groove_page = self.groove_page.load(Ordering::Relaxed);
        let follow = self.mk3_follow_playhead.load(Ordering::Relaxed);
        s.groove_follow_playhead = follow;
        s.groove_event_mode = self.mk3_event_mode.load(Ordering::Relaxed);
        s.groove_sel_pad = self.groove_sel_pad.load(Ordering::Relaxed).min(24);
        // window_layout ya vive en SessionState y se guarda tal cual.

        // Copia de trabajo: lo que se escribe lleva rutas de audio relativas; el estado
        // vivo conserva rutas absolutas.
        let mut out = s.clone();
        out.format_version = lanzadera_core::FORMAT_VERSION;
        let mut moved: Vec<(usize, usize, String)> = Vec::new();
        if final_save {
            for (t, tr) in out.tracks.iter_mut().enumerate() {
                for (sc, slot) in tr.clips.iter_mut().enumerate() {
                    let Some(c) = slot else { continue };
                    let Some(p) = c.audio_path.clone() else { continue };
                    let src = std::path::PathBuf::from(&p);
                    let src_abs = if src.is_absolute() { src } else { dir.join(src) };
                    if src_abs.starts_with(&audio_dir) {
                        // Ya vive en la carpeta del proyecto: solo se relativiza.
                        if let Ok(rel) = src_abs.strip_prefix(&dir) {
                            c.audio_path = Some(rel.to_string_lossy().into_owned());
                        }
                        continue;
                    }
                    if !src_abs.is_file() {
                        eprintln!("[proyecto] audio no encontrado, se guarda la ruta tal cual: {}", p);
                        continue;
                    }
                    std::fs::create_dir_all(&audio_dir)
                        .map_err(|e| format!("No se pudo crear {}: {}", audio_dir.display(), e))?;
                    let base = src_abs.file_name().and_then(|n| n.to_str()).unwrap_or("audio.wav");
                    let dest_name = format!("p{}_e{}_{}", t + 1, sc + 1, base);
                    let dest = audio_dir.join(&dest_name);
                    std::fs::copy(&src_abs, &dest)
                        .map_err(|e| format!("No se pudo copiar {}: {}", src_abs.display(), e))?;
                    c.audio_path = Some(format!("{}/{}", audio_dir_name, dest_name));
                    moved.push((t, sc, dest.to_string_lossy().into_owned()));
                }
            }
        }

        let json = out.to_json();
        if final_save {
            let tmp = format!("{}.tmp", path);
            std::fs::write(&tmp, &json).map_err(|e| e.to_string())?;
            if proj.exists() {
                let _ = std::fs::rename(path, format!("{}.bak", path));
            }
            std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
            if let Ok(mut persist) = self.persist.lock() {
                *persist = json.clone();
            }
            // El estado vivo apunta ya a la copia dentro del proyecto (no a /tmp).
            for (t, sc, p) in moved {
                if let Some(Some(c)) = s.tracks.get_mut(t).and_then(|tr| tr.clips.get_mut(sc)) {
                    c.audio_path = Some(p);
                }
            }
            Ok(())
        } else {
            std::fs::write(path, &json).map_err(|e| e.to_string())
        }
    }

    /// Huella del contenido guardable del proyecto: ignora lo transitorio (transporte,
    /// estado de los clips, selección, navegación del Groove). Sirve para saber si hay
    /// cambios sin guardar comparando dos huellas.
    pub fn project_fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut s = { self.state.lock().unwrap_or_else(|e| e.into_inner()).clone() };
        s.playing = false;
        s.recording = false;
        s.selected_track = 0;
        s.selected_scene = 0;
        s.selected_clip = None;
        s.groove_json = self.groove.to_json();
        s.groove_view = 0;
        s.groove_bar = 0;
        s.groove_page = 0;
        s.groove_sel_pad = 0;
        s.groove_event_mode = false;
        s.window_layout = Default::default();
        for tr in s.tracks.iter_mut() {
            for slot in tr.clips.iter_mut() {
                if let Some(c) = slot { c.state = ClipState::Empty; }
            }
            // El estado de los plugins se captura a destiempo: no cuenta como edición.
            tr.plugin_state = None;
            tr.note_fx_state = None;
            tr.fx_state = None;
            for slot in tr.note_fx_chain.iter_mut() { slot.state = None; }
            for slot in tr.fx_chain.iter_mut() { slot.state = None; }
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        s.to_json().hash(&mut h);
        h.finish()
    }

    /// Vacía los almacenes de audio y notas de todos los slots (proyecto nuevo/cargado).
    fn clear_all_slot_stores(&self) {
        for t in 0..MAX_TRACKS {
            for sc in 0..MAX_SCENES {
                self.samples.clear(t, sc);
                self.notes.clear(t, sc);
            }
        }
    }

    /// Proyecto nuevo: sesión por defecto y patrón de Groove inicial. No reinicia el
    /// audio: quien llama activa `audio_restart` para que se descarguen los plugins.
    pub fn new_project(&self) {
        let s = SessionState::default();
        if let Some(j) = GROOVE_DEFAULT_JSON.get() { let _ = self.groove.load_json(j); }
        self.groove_view.store(s.groove_view.min(1), Ordering::Relaxed);
        self.groove_bar.store(s.groove_bar, Ordering::Relaxed);
        self.groove_page.store(s.groove_page, Ordering::Relaxed);
        self.groove_sel_pad.store(s.groove_sel_pad.min(24), Ordering::Relaxed);
        self.mk3_event_mode.store(s.groove_event_mode, Ordering::Relaxed);
        self.mk3_follow_playhead.store(s.groove_follow_playhead, Ordering::Relaxed);
        self.groove_follow_playhead.store(s.groove_follow_playhead, Ordering::Relaxed);
        self.clear_all_slot_stores();
        self.edit(|st| *st = s);
    }

    pub fn load_from_file(&self, path: &str) -> Result<(), String> {
        let json = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let mut s = SessionState::from_json(&json)?;
        // Las rutas de audio relativas se resuelven contra la carpeta del proyecto.
        let base_dir = std::path::Path::new(path).parent().filter(|d| !d.as_os_str().is_empty())
            .map(|d| d.to_path_buf()).unwrap_or_else(|| std::path::PathBuf::from("."));

        // Cargar el patrón antes de cambiar el resto de la sesión: si el JSON
        // del Groove está corrupto, devolver el error sin sustituir el proyecto.
        if !s.groove_json.is_empty() {
            self.groove.load_json(&s.groove_json)?;
        }

        self.groove_view.store(s.groove_view.min(1), Ordering::Relaxed);
        self.groove_bar.store(s.groove_bar, Ordering::Relaxed);
        self.groove_page.store(s.groove_page, Ordering::Relaxed);
        self.groove_sel_pad.store(s.groove_sel_pad.min(24), Ordering::Relaxed);
        self.mk3_event_mode.store(s.groove_event_mode, Ordering::Relaxed);
        self.mk3_follow_playhead.store(s.groove_follow_playhead, Ordering::Relaxed);
        self.groove_follow_playhead.store(s.groove_follow_playhead, Ordering::Relaxed);

        let mut loads = Vec::new();
        let mut midi = Vec::new();
        for (t, tr) in s.tracks.iter_mut().enumerate() {
            for (sc, slot) in tr.clips.iter_mut().enumerate() {
                if let Some(c) = slot {
                    c.state = ClipState::Empty;
                    if let Some(p) = c.audio_path.clone() {
                        let abs = if std::path::Path::new(&p).is_absolute() { p }
                                  else { base_dir.join(&p).to_string_lossy().into_owned() };
                        c.audio_path = Some(abs.clone());
                        loads.push((t, sc, abs));
                    }
                    if !c.midi_notes.is_empty() { midi.push((t, sc)); }
                }
            }
        }
        s.playing = false;
        // Migración (Paso 2): proyectos con Note FX / FX de slot único → cadenas de 1 slot.
        for tr in s.tracks.iter_mut() {
            if tr.note_fx_chain.is_empty() {
                if let Some(p) = tr.note_fx_path.take() {
                    tr.note_fx_chain.push(lanzadera_core::track::PluginSlot {
                        path: p, id: tr.note_fx_id.take(), state: tr.note_fx_state.take(), bypass: false,
                    });
                }
            }
            if tr.fx_chain.is_empty() {
                if let Some(p) = tr.fx_path.take() {
                    tr.fx_chain.push(lanzadera_core::track::PluginSlot {
                        path: p, id: tr.fx_id.take(), state: tr.fx_state.take(), bypass: false,
                    });
                }
            }
        }
        self.clear_all_slot_stores();
        self.edit(|st| *st = s);
        for (t, sc) in midi { self.sync_midi(t, sc); }
        for (t, sc, p) in loads { self.request_load(t, sc, p); }
        Ok(())
    }
}

// ════════════════════════════════════════════════════════════════════════
//  Modo CHORDS (Library) — dado, locks, pin de muestra y favoritos.
//  Toda la lógica vive aquí para que el controlador y la UI usen exactamente
//  el mismo camino sin depender de caja-core directamente.
// ════════════════════════════════════════════════════════════════════════
impl SessionShared {
    /// Banco de muestras de una celda (rutas relativas, orden estable).
    pub fn chords_bank(&self, pad: usize) -> Vec<String> {
        let pad = pad.min(NUM_PADS - 1);
        let lib = crate::caja_drums::library_state::shared();
        let g = lib.lock().unwrap_or_else(|e| e.into_inner());
        g.lib.bank(pad, g.bank_cap).to_vec()
    }

    /// Índice de la muestra que suena ahora: el del último golpe si ya sonó,
    /// si no el que daría la semilla actual.
    fn chords_index(&self, pad: usize, bank_len: usize) -> Option<usize> {
        if bank_len == 0 { return None; }
        let idx = self.caja_store.cur_idx[pad].load(Ordering::Relaxed);
        if idx >= 0 && (idx as usize) < bank_len { return Some(idx as usize); }
        self.kit.seed_sample_index(pad, bank_len)
    }

    /// Muestra que está sonando en la celda (ruta relativa).
    pub fn chords_current_sample(&self, pad: usize) -> Option<String> {
        let pad = pad.min(NUM_PADS - 1);
        let bank = self.chords_bank(pad);
        let i = self.chords_index(pad, bank.len())?;
        bank.get(i).cloned()
    }

    /// Fija una muestra concreta del banco: pin + lock de muestra.
    pub fn chords_pin(&self, pad: usize, rel: &str) -> bool {
        let pad = pad.min(NUM_PADS - 1);
        let bank = self.chords_bank(pad);
        let Some(idx) = bank.iter().position(|r| r == rel) else { return false };
        self.kit.set_pin(pad, Some(rel.to_string()));
        self.kit.set_lock(pad, P_SAMPLE, true);
        self.caja_store.pin_idx[pad].store(idx as i32, Ordering::Relaxed);
        self.caja_store.pin_missing[pad].store(false, Ordering::Relaxed);
        let lib = crate::caja_drums::library_state::shared();
        let mut g = lib.lock().unwrap_or_else(|e| e.into_inner());
        g.set_pin(pad, Some((idx, rel.to_string())));
        g.save();
        true
    }

    /// Fija la muestra que está sonando ahora.
    pub fn chords_pin_current(&self, pad: usize) -> Option<String> {
        let rel = self.chords_current_sample(pad)?;
        self.chords_pin(pad, &rel).then_some(rel)
    }

    /// Quita el pin y el lock de muestra.
    pub fn chords_unpin(&self, pad: usize) {
        let pad = pad.min(NUM_PADS - 1);
        self.kit.set_pin(pad, None);
        self.kit.set_lock(pad, P_SAMPLE, false);
        self.caja_store.pin_idx[pad].store(-1, Ordering::Relaxed);
        self.caja_store.pin_missing[pad].store(false, Ordering::Relaxed);
        let lib = crate::caja_drums::library_state::shared();
        let mut g = lib.lock().unwrap_or_else(|e| e.into_inner());
        g.set_pin(pad, None);
        g.save();
    }

    /// Dado: semilla nueva y pin liberado, para que el golpe estrene muestra.
    pub fn chords_roll(&self, pad: usize) {
        self.chords_unpin(pad);
        self.kit.roll_seed(pad.min(NUM_PADS - 1));
    }

    /// Alterna un lock. Al bloquear Muestra se fija el pin; al liberarlo se
    /// quita (0 = Muestra, 1 = Pitch, 2 = Decay, 3 = Filtro).
    pub fn chords_toggle_lock(&self, pad: usize, param: usize) -> bool {
        let on = self.kit.toggle_lock(pad.min(NUM_PADS - 1), param);
        if param == P_SAMPLE {
            if on { self.chords_pin_current(pad); } else { self.chords_unpin(pad); }
        }
        on
    }

    /// ALL ROLL: las 25 celdas vuelven a tirar el dado en cada golpe.
    pub fn chords_all_roll(&self) {
        self.kit.set_all_modes(ALL_ROLL);
    }

    /// ALL LOCK: bloquea los 4 parámetros de las 25 celdas y fija la muestra
    /// que está sonando en cada una.
    pub fn chords_all_lock(&self) {
        let mut pins: Vec<Option<(usize, String)>> = Vec::new();
        {
            let lib = crate::caja_drums::library_state::shared();
            let g = lib.lock().unwrap_or_else(|e| e.into_inner());
            for pad in 0..NUM_PADS {
                let bank = g.lib.bank(pad, g.bank_cap);
                pins.push(
                    self.chords_index(pad, bank.len())
                        .and_then(|i| bank.get(i).cloned().map(|rel| (i, rel))),
                );
            }
        }
        self.kit.set_all_modes(ALL_LOCK);
        let lib = crate::caja_drums::library_state::shared();
        let mut g = lib.lock().unwrap_or_else(|e| e.into_inner());
        for (pad, pin) in pins.into_iter().enumerate() {
            let idx = pin.as_ref().map(|(i, _)| *i).unwrap_or(usize::MAX);
            self.caja_store.pin_idx[pad].store(
                if pin.is_some() { idx as i32 } else { -1 },
                Ordering::Relaxed,
            );
            self.caja_store.pin_missing[pad].store(false, Ordering::Relaxed);
            self.kit.set_pin(pad, pin.as_ref().map(|(_, rel)| rel.clone()));
            g.set_pin(pad, pin);
        }
        g.save();
    }

    /// "Todos 4c": alterna el re-dado cada `bars` compases en todas las
    /// celdas. Devuelve el rate resultante (0 = OFF).
    pub fn chords_toggle_rate(&self, bars: u8) -> u8 {
        let cfg = self.kit.snapshot_cfg();
        let all = cfg.iter().all(|c| c.rate == bars);
        let next = if all { 0 } else { bars };
        self.kit.set_all_rates(next);
        next
    }

    // ── Favoritos de muestras ─────────────────────────────────────────

    pub fn chords_toggle_favorite(&self, rel: &str) -> bool {
        let lib = crate::caja_drums::library_state::shared();
        let on = lib.lock().unwrap_or_else(|e| e.into_inner()).toggle_fav(rel);
        on
    }

    pub fn chords_is_favorite(&self, rel: &str) -> bool {
        let lib = crate::caja_drums::library_state::shared();
        let fav = lib.lock().unwrap_or_else(|e| e.into_inner()).is_fav(rel);
        fav
    }

    pub fn chords_favorites(&self) -> Vec<String> {
        let lib = crate::caja_drums::library_state::shared();
        let list = lib.lock().unwrap_or_else(|e| e.into_inner()).favs.clone();
        list
    }
}

#[cfg(test)]
mod chords_shared_tests {
    use super::*;

    #[test]
    fn sin_banco_no_hay_muestra_que_fijar() {
        let shared = SessionShared::new();
        assert!(shared.chords_current_sample(0).is_none());
        assert!(shared.chords_pin_current(0).is_none());
        // No debe entrar en pánico ni dejar el pin marcado.
        assert_eq!(shared.caja_store.pin_idx[0].load(Ordering::Relaxed), -1);
    }

    #[test]
    fn fijar_una_muestra_activa_el_lock_de_muestra() {
        let shared = SessionShared::new();
        // El banco real lo llena el loader; aquí solo se verifica el camino
        // de lock/unlock sobre el kit.
        // El kit arranca en ALL LOCK: fijamos un punto de partida conocido.
        shared.chords_all_roll();
        assert!(shared.chords_toggle_lock(1, P_SAMPLE));
        assert!(caja_core::kit::is_lock(shared.kit.snapshot_live()[1].mode, P_SAMPLE));
        assert!(!shared.chords_toggle_lock(1, P_SAMPLE));
        assert!(!caja_core::kit::is_lock(shared.kit.snapshot_live()[1].mode, P_SAMPLE));
    }

    #[test]
    fn all_lock_y_all_roll_cambian_el_modo_global() {
        let shared = SessionShared::new();
        shared.chords_all_lock();
        assert!(shared.kit.snapshot_live().iter().all(|l| l.mode == ALL_LOCK));
        shared.chords_all_roll();
        assert!(shared.kit.snapshot_live().iter().all(|l| l.mode == ALL_ROLL));
    }

    #[test]
    fn el_rate_se_alterna_entre_cero_y_cuatro_compases() {
        let shared = SessionShared::new();
        assert_eq!(shared.chords_toggle_rate(4), 4);
        assert!(shared.kit.snapshot_cfg().iter().all(|c| c.rate == 4));
        assert_eq!(shared.chords_toggle_rate(4), 0);
        assert!(shared.kit.snapshot_cfg().iter().all(|c| c.rate == 0));
    }
}

#[cfg(test)]
mod groove_preset_tests {
    use super::*;

    #[test]
    fn guardar_listar_cargar_y_borrar_preset() {
        let shared = SessionShared::new();
        shared.save_groove_preset("  Funk A  ").unwrap();
        assert_eq!(shared.list_groove_presets(), vec!["Funk A".to_string()]);
        // Mismo nombre sin distinguir mayúsculas: reemplaza, no duplica.
        shared.save_groove_preset("funk a").unwrap();
        assert_eq!(shared.list_groove_presets().len(), 1);
        assert!(shared.load_groove_preset("FUNK A").is_ok());
        assert!(shared.load_groove_preset("no existe").is_err());
        assert!(shared.save_groove_preset("   ").is_err());
        shared.delete_groove_preset("Funk A").unwrap();
        assert!(shared.list_groove_presets().is_empty());
        assert!(shared.delete_groove_preset("Funk A").is_err());
    }

    #[test]
    fn presets_viajan_en_el_json_del_proyecto() {
        let shared = SessionShared::new();
        shared.save_groove_preset("Intro").unwrap();
        let json = shared.state.lock().unwrap().to_json();
        let back = SessionState::from_json(&json).unwrap();
        assert_eq!(back.groove_presets.len(), 1);
        assert_eq!(back.groove_presets[0].name, "Intro");
    }

    #[test]
    fn vaciar_slot_no_borra_el_preset_global() {
        let shared = SessionShared::new();
        shared.save_groove_preset_slot_named(0, "Bajo funky").unwrap();
        assert!(shared.clear_groove_preset_slot(0).is_ok());
        assert!(shared.state.lock().unwrap().groove_preset_slots[0].is_none());
        assert!(shared.list_groove_presets().iter().any(|n| n == "Bajo funky"));
        assert!(shared.load_groove_preset("Bajo funky").is_ok());
    }

    #[test]
    fn peticiones_mk3_de_presets_se_acumulan_y_se_consumen_una_vez() {
        let shared = SessionShared::new();

        shared.request_mk3_preset_load(0);
        shared.request_mk3_preset_load(0); // Repetir el mismo slot no duplica el bit.
        shared.request_mk3_preset_load(3);
        shared.request_mk3_preset_save(2);

        assert_eq!(shared.take_mk3_preset_requests(), (0b0000_1001, 0b0000_0100));
        assert_eq!(shared.take_mk3_preset_requests(), (0, 0));

        // Slots fuera de rango se ignoran.
        shared.request_mk3_preset_load(8);
        shared.request_mk3_preset_save(usize::MAX);
        assert_eq!(shared.take_mk3_preset_requests(), (0, 0));
    }

    #[test]
    fn slots_mk3_se_conservan_al_serializar_la_sesion() {
        let shared = SessionShared::new();
        shared.save_groove_preset_slot(0).unwrap();
        shared.save_groove_preset_slot(7).unwrap();

        let json = shared.state.lock().unwrap().to_json();
        let restored = SessionState::from_json(&json).unwrap();

        assert_eq!(restored.groove_preset_slots[0].as_deref(), Some("MK3 Slot 1"));
        assert_eq!(restored.groove_preset_slots[7].as_deref(), Some("MK3 Slot 8"));
        assert_eq!(restored.groove_preset_active, 7);
        assert!(restored.groove_presets.iter().any(|preset| preset.name == "MK3 Slot 1"));
        assert!(restored.groove_presets.iter().any(|preset| preset.name == "MK3 Slot 8"));
    }
}

impl SessionShared {
    /// Publica la lista de parámetros del plugin para la UI.
    /// Formato compatible con `Track::plugin_params`: (id, nombre, valor 0..1).
    pub fn set_plugin_params(&self, t: usize, params: Vec<(String, String, f32)>) {
        self.edit(|st| {
            if let Some(tr) = st.tracks.get_mut(t) {
                tr.plugin_params = params;
            }
        });
    }

    /// Envía un cambio de parámetro al hilo de audio sin pasar por el reductor
    /// de sesión completo. Usa `live_rx`, que ya es un canal SPSC.
    pub fn queue_plugin_param(&self, track: usize, param_idx: usize, value: f32) {
        let cmd = ControlCmd::SetPluginParam {
            track: track as u8,
            param_idx: param_idx as u8,
            value,
        };
        let _ = self.live_rx.push(cmd.encode());
    }
}
