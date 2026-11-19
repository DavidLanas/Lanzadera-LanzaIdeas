//! Hosting de plugins VST3 (Fase 7) usando vst3-host 0.11 (MIT).
//!
//! # Atribución
//!   - vst3-host: https://github.com/HelgeSverre/rust-vst3-host (MIT)
//!   - VST3 SDK: Steinberg (MIT desde 2025)
//!
//! # API verificada en source de vst3-host 0.11.1
//!   - simple::load_plugin(path) -> Result<Plugin> (no requiere guardar Vst3Host)
//!   - plugin.get_parameters() -> Result<Vec<Parameter>>
//!   - plugin.start_processing() -> Result<()>
//!   - plugin.process_bus_audio(&mut BusAudioBuffers) -> Result<()>
//!   - plugin.send_midi_event_at(MidiEvent, sample_offset) -> Result<()>
//!   - plugin.set_parameter_at(id, value, sample_offset) -> Result<()>

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use vst3_host::{
    audio::BusAudioBuffers,
    midi::{MidiChannel, MidiEvent},
    plugin::Plugin,
    simple,
    PluginWindow,
};

use crate::plugin_events::{EvKind, PluginEv, MAX_PLUGIN_EVENTS};

/// (ruta, nombre) de un plugin VST3 encontrado por `scan`.
pub type Vst3Ref = (String, String);

/// Información de un parámetro VST3 expuesto por el plugin.
#[derive(Clone, Debug)]
pub struct Vst3ParamInfo {
    pub id: u32,
    pub name: String,
    pub value: f32,  // normalizado 0-1
    pub min: f64,
    pub max: f64,
}

/// Slot de plugin VST3 para una pista.
///
/// `plugin` va en `Arc<Mutex<..>>` porque la GUI nativa (`PluginWindow`, hilo
/// principal) y el procesado de audio (callback RT) tocan el MISMO objeto
/// `Plugin`, igual que hace `simple::play` de vst3-host. El hilo de audio
/// NUNCA bloquea: usa `try_lock` con un spin acotado (`lock_for_audio`).
pub struct Vst3Slot {
    pub plugin: Arc<Mutex<Plugin>>,
    pub buffers: BusAudioBuffers,  // reutilizar en cada frame
    pub params: Vec<Vst3ParamInfo>,
    pub is_instrument: bool,  // true si category == "Instrument"
    pub name: String,
    pub has_editor: bool,
    /// Eventos que no se pudieron entregar porque la GUI tenia el lock en
    /// ese bloque (evita notas colgadas por un NoteOff perdido). Capacidad
    /// fija reservada en `load`: sin alloc en RT.
    pending: Vec<PluginEv>,
}

/// Directorios estándar de VST3 en Linux (mas `$VST3_PATH`, separado por ':').
fn vst3_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        v.push(PathBuf::from(home).join(".vst3"));
    }
    v.push("/usr/lib/vst3".into());
    v.push("/usr/lib64/vst3".into());
    v.push("/usr/local/lib/vst3".into());
    if let Some(extra) = std::env::var_os("VST3_PATH") {
        for p in std::env::split_paths(&extra) { v.push(p); }
    }
    v
}

/// En Linux un `.vst3` es un BUNDLE (directorio `Foo.vst3/Contents/x86_64-linux/Foo.so`),
/// no un archivo. El escaneo anterior comprobaba `is_dir()` ANTES que la
/// extension y entraba dentro del bundle sin listarlo nunca. Ahora la
/// extension manda y no se desciende dentro de un `.vst3`.
fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: usize,
        seen: &mut std::collections::HashSet<PathBuf>)
{
    if depth > 6 { return; }
    // canonicalize: evita bucles por symlinks a un ancestro.
    let Ok(real) = dir.canonicalize() else { return };
    if !seen.insert(real) { return; }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) == Some("vst3") {
            out.push(p);
        } else if p.is_dir() {
            walk(&p, out, depth + 1, seen);
        }
    }
}

/// Escanea los directorios estándar y devuelve (ruta_del_bundle, nombre).
pub fn scan() -> Vec<Vst3Ref> {
    let mut paths = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for d in vst3_dirs() {
        if d.exists() { walk(&d, &mut paths, 0, &mut seen); }
    }
    paths.sort();
    paths.dedup();
    let mut out: Vec<Vst3Ref> = paths.into_iter().filter_map(|p| {
        let name = p.file_stem()?.to_str()?.to_string();
        Some((p.to_string_lossy().into_owned(), name))
    }).collect();
    out.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    out
}

/// Acepta la ruta del bundle, o un archivo de dentro (`.../Foo.so`), o lo que
/// el usuario haya escrito a mano, y devuelve el `.vst3` que hay que cargar.
pub fn resolve_bundle(path: &str) -> PathBuf {
    let p = Path::new(path.trim());
    for a in p.ancestors() {
        if a.extension().and_then(|x| x.to_str()) == Some("vst3") {
            return a.to_path_buf();
        }
    }
    p.to_path_buf()
}

/// Carga y activa un plugin VST3. Hilo principal (los plugins VST3 exigen que
/// la creacion/teardown ocurran fuera del hilo de audio).
pub fn load(path: &str, sample_rate: u32, max_frames: u32)
    -> Result<Vst3Slot, String>
{
    let bundle = resolve_bundle(path);
    if !bundle.exists() {
        return Err(format!("no existe: {}", bundle.display()));
    }
    let mut plugin = simple::load_plugin_with_settings(&bundle, sample_rate as f64, max_frames as usize)
        .map_err(|e| format!("load_plugin({}): {}", bundle.display(), e))?;

    let info = plugin.info();
    let is_instrument = info.category.to_lowercase().contains("instrument");
    let name = info.name.clone();

    // Crear buffers de audio reutilizables.
    let buffers = plugin.create_bus_audio_buffers(max_frames as usize)
        .map_err(|e| format!("create_bus_audio_buffers: {}", e))?;

    let params = describe_params(&plugin);
    let has_editor = plugin.has_editor();

    plugin.start_processing()
        .map_err(|e| format!("start_processing: {}", e))?;

    Ok(Vst3Slot {
        plugin: Arc::new(Mutex::new(plugin)),
        buffers, params, is_instrument, name, has_editor,
        pending: Vec::with_capacity(MAX_PLUGIN_EVENTS * 2),
    })
}

/// Enumera los parámetros expuestos por el plugin.
pub fn describe_params(plugin: &Plugin) -> Vec<Vst3ParamInfo> {
    let mut out = Vec::new();
    if let Ok(params) = plugin.get_parameters() {
        for param in params {
            out.push(Vst3ParamInfo {
                id: param.id,
                name: param.name.clone(),
                value: param.value as f32,
                min: param.min,
                max: param.max,
            });
        }
    }
    out
}

/// Convierte u8 channel (0-15) a MidiChannel enum.
pub fn to_midi_channel(ch: u8) -> MidiChannel {
    match ch {
        0 => MidiChannel::Ch1,
        1 => MidiChannel::Ch2,
        2 => MidiChannel::Ch3,
        3 => MidiChannel::Ch4,
        4 => MidiChannel::Ch5,
        5 => MidiChannel::Ch6,
        6 => MidiChannel::Ch7,
        7 => MidiChannel::Ch8,
        8 => MidiChannel::Ch9,
        9 => MidiChannel::Ch10,
        10 => MidiChannel::Ch11,
        11 => MidiChannel::Ch12,
        12 => MidiChannel::Ch13,
        13 => MidiChannel::Ch14,
        14 => MidiChannel::Ch15,
        _ => MidiChannel::Ch16,
    }
}

/// Lock para el hilo de audio: NUNCA bloquea. Reintenta unos microsegundos
/// (la GUI solo retiene el lock durante `service_run_loop`) y si no lo logra
/// devuelve None; el llamador conserva los eventos para el bloque siguiente.
#[inline]
fn lock_for_audio(m: &Mutex<Plugin>) -> Option<std::sync::MutexGuard<'_, Plugin>> {
    use std::sync::TryLockError;
    let t0 = std::time::Instant::now();
    loop {
        match m.try_lock() {
            Ok(g) => return Some(g),
            Err(TryLockError::Poisoned(p)) => return Some(p.into_inner()),
            Err(TryLockError::WouldBlock) => {
                if t0.elapsed().as_micros() > 300 { return None; }
                std::hint::spin_loop();
            }
        }
    }
}

/// Traduce un evento de la cola neutral y lo entrega al plugin.
fn deliver(plugin: &mut Plugin, ev: &PluginEv) {
    let frame = ev.frame as i32;
    match ev.kind {
        EvKind::NoteOn => {
            let _ = plugin.send_midi_event_at(
                MidiEvent::NoteOn { channel: to_midi_channel(ev.c), note: ev.a, velocity: ev.b }, frame);
        }
        EvKind::NoteOff => {
            let _ = plugin.send_midi_event_at(
                MidiEvent::NoteOff { channel: to_midi_channel(ev.c), note: ev.a, velocity: 0 }, frame);
        }
        EvKind::Midi3 => {
            // Midi3: a = status, b = data1, c = data2. (El codigo anterior
            // usaba `c` como canal, que aqui es data2.)
            let ch = to_midi_channel(ev.a & 0x0F);
            let e = match ev.a & 0xF0 {
                0xB0 => Some(MidiEvent::ControlChange { channel: ch, controller: ev.b, value: ev.c }),
                0xE0 => Some(MidiEvent::PitchBend {
                    channel: ch, value: ((ev.c as u16 & 0x7F) << 7) | (ev.b as u16 & 0x7F) }),
                _ => None,
            };
            if let Some(e) = e { let _ = plugin.send_midi_event_at(e, frame); }
        }
        EvKind::Param => {
            let _ = plugin.set_parameter_at(ev.id, ev.plain.clamp(0.0, 1.0), frame);
        }
    }
}

/// Procesa UN bloque: entrega eventos + `process_bus_audio`. RT-safe (sin
/// alloc, sin bloqueo). Devuelve true si el plugin proceso y `slot.buffers`
/// tiene la salida; false si la GUI tenia el lock (el bloque se omite y los
/// eventos se aplazan).
pub fn process(slot: &mut Vst3Slot, events: &[PluginEv], frames: usize) -> bool {
    let Some(mut plugin) = lock_for_audio(&slot.plugin) else {
        for e in events {
            if slot.pending.len() < slot.pending.capacity() {
                let mut e = *e;
                e.frame = 0;
                slot.pending.push(e);
            }
        }
        return false;
    };
    // El plugin procesa tantas muestras como midan los canales. Los buffers
    // se crearon con MAX_BLOCK (4096): sin ajustarlos, cada callback de, p. ej.,
    // 512 muestras hacia procesar 4096 -> sonido comprimido/saturado "como con
    // poco buffer". resize dentro de la capacidad reservada no asigna memoria.
    let n = frames.clamp(1, slot.buffers.block_size.max(1));
    for bus in slot.buffers.inputs.iter_mut().chain(slot.buffers.outputs.iter_mut()) {
        for ch in bus.channels.iter_mut() {
            if ch.len() != n { ch.resize(n, 0.0); }
        }
    }
    slot.buffers.clear();
    for e in slot.pending.iter() { deliver(&mut plugin, e); }
    slot.pending.clear();
    for e in events { deliver(&mut plugin, e); }
    plugin.process_bus_audio(&mut slot.buffers).is_ok()
}

// ───────────────────────── GUI nativa VST3 ─────────────────────────

/// Ventanas del editor VST3 abiertas, una por pista. Hilo principal.
///
/// Usa `vst3_host::PluginWindow` (MIT): crea una ventana X11 y embebe la vista
/// del plugin (`X11EmbedWindowID`). Las GUI VSTGUI de Linux solo se dibujan si
/// el host llama a `service_run_loop` cada frame: `poll` lo hace.
#[derive(Default)]
pub struct Vst3Gui {
    wins: Vec<(usize, PluginWindow)>,
}

impl Vst3Gui {
    pub fn is_open(&self, track: usize) -> bool { self.wins.iter().any(|(t, _)| *t == track) }

    /// Abre o cierra el editor de la pista. Devuelve el nuevo estado `abierta`.
    pub fn toggle(&mut self, track: usize, plugin: &Arc<Mutex<Plugin>>) -> Result<bool, String> {
        if self.is_open(track) {
            self.close(track);
            return Ok(false);
        }
        let mut w = PluginWindow::new(plugin.clone());
        w.open().map_err(|e| format!("abrir editor VST3: {}", e))?;
        self.wins.push((track, w));
        Ok(true)
    }

    pub fn close(&mut self, track: usize) {
        // Drop de PluginWindow = close(): suelta la vista del plugin y luego
        // destruye la ventana padre, en ese orden.
        self.wins.retain(|(t, _)| *t != track);
    }

    pub fn close_all(&mut self) { self.wins.clear(); }

    /// Cada frame: da cuerda al run loop del plugin, aplica resizes y detecta
    /// cierre por la X. Devuelve las pistas cuyo editor se cerro.
    pub fn poll(&mut self, plugins: &[(usize, Arc<Mutex<Plugin>>)]) -> Vec<usize> {
        let mut closed = Vec::new();
        for (t, w) in self.wins.iter() {
            if w.closed_by_user() { closed.push(*t); continue; }
            let _ = w.service_platform_events();
            if let Some((_, p)) = plugins.iter().find(|(i, _)| i == t) {
                // try_lock: si el audio esta procesando, se salta este frame.
                if let Ok(mut g) = p.try_lock() { g.service_run_loop(); }
            }
        }
        for t in &closed { self.close(*t); }
        closed
    }
}
