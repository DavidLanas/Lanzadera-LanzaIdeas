//! Host CLAP sobre clack-host 0.2 + servicios de main-thread (paso 1).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use clack_host::prelude::*;
use clack_extensions::params::{ParamInfoBuffer, ParamInfoFlags, PluginParams};
use clack_extensions::gui::{GuiSize, HostGui, HostGuiImpl};
use clack_extensions::timer::{HostTimer, HostTimerImpl, TimerId};
use clack_extensions::thread_check::{HostThreadCheck, HostThreadCheckImpl};
use clack_extensions::state::{HostState, HostStateImpl};
#[cfg(target_os = "linux")]
use clack_extensions::posix_fd::{FdFlags, HostPosixFd, HostPosixFdImpl};
#[cfg(target_os = "linux")]
use std::os::unix::io::RawFd;

pub struct LanzaderaShared { pub track: usize }

impl<'a> SharedHandler<'a> for LanzaderaShared {
    fn request_restart(&self)  { crate::clap_services::request_restart(self.track); }
    fn request_process(&self)  { crate::clap_services::request_process(self.track); }
    fn request_callback(&self) { crate::clap_services::request_callback(self.track); }
}

impl HostThreadCheckImpl for LanzaderaShared {
    fn is_main_thread(&self) -> bool { crate::clap_services::is_main_thread() }
    fn is_audio_thread(&self) -> bool { crate::clap_services::is_audio_thread() }
}

impl HostGuiImpl for LanzaderaShared {
    fn resize_hints_changed(&self) {}
    fn request_resize(&self, new_size: GuiSize) -> Result<(), HostError> {
        crate::clap_gui::note_resize(self.track, new_size.width, new_size.height);
        Ok(())
    }
    fn request_show(&self) -> Result<(), HostError> { Ok(()) }
    fn request_hide(&self) -> Result<(), HostError> { Ok(()) }
    fn closed(&self, was_destroyed: bool) {
        crate::clap_gui::note_closed(self.track, was_destroyed);
    }
}

pub struct LanzaderaMainThread { pub track: usize }
impl LanzaderaMainThread {
    pub fn new(track: usize) -> Self {
        crate::clap_services::mark_main_thread();
        Self { track }
    }
}
impl<'a> MainThreadHandler<'a> for LanzaderaMainThread {}

impl HostTimerImpl for LanzaderaMainThread {
    fn register_timer(&self, period_ms: u32) -> Result<TimerId, HostError> {
        Ok(TimerId(crate::clap_services::register_timer(self.track, period_ms)))
    }
    fn unregister_timer(&self, timer_id: TimerId) -> Result<(), HostError> {
        crate::clap_services::unregister_timer(self.track, timer_id.0);
        Ok(())
    }
}

#[cfg(target_os = "linux")]
impl HostPosixFdImpl for LanzaderaMainThread {
    fn register_fd(&self, fd: RawFd, flags: FdFlags) -> Result<(), HostError> {
        crate::clap_services::fds::register(self.track, fd, flags); Ok(())
    }
    fn modify_fd(&self, fd: RawFd, flags: FdFlags) -> Result<(), HostError> {
        crate::clap_services::fds::register(self.track, fd, flags); Ok(())
    }
    fn unregister_fd(&self, fd: RawFd) -> Result<(), HostError> {
        crate::clap_services::fds::unregister(self.track, fd); Ok(())
    }
}

impl HostStateImpl for LanzaderaMainThread {
    fn mark_dirty(&self) {
        crate::clap_services::mark_dirty(self.track);
        crate::state_ctl::mark_plugin_dirty();
    }
}

pub struct LanzaderaHost;
impl HostHandlers for LanzaderaHost {
    type Shared<'a> = LanzaderaShared;
    type MainThread<'a> = LanzaderaMainThread;
    type AudioProcessor<'a> = ();

    fn declare_extensions(builder: &mut HostExtensions<Self>, _shared: &Self::Shared<'_>) {
        builder
            .register::<HostGui>()
            .register::<HostTimer>()
            .register::<HostThreadCheck>()
            .register::<HostState>();
        #[cfg(target_os = "linux")]
        builder.register::<HostPosixFd>();
    }
}

pub type Instance = PluginInstance<LanzaderaHost>;
pub type Processor = PluginAudioProcessor<LanzaderaHost>;
pub type Stopped = clack_host::process::StoppedPluginAudioProcessor<LanzaderaHost>;

/// (ruta, id, nombre) de un plugin encontrado por scan.
pub type ClapRef = (String, String, String);

/// Información de un parámetro CLAP expuesto por el plugin.
///
/// API basada en clack-extensions (MIT/Apache). La normalización 0..1 sigue
/// la convención habitual de CLAP para automatización/control desde host.
#[derive(Clone, Debug)]
pub struct PluginParamInfo {
    pub id: u32,
    pub name: String,
    pub value: f32,
    pub min: f64,
    pub max: f64,
}

static ENTRIES: OnceLock<Mutex<HashMap<String, &'static PluginEntry>>> = OnceLock::new();

fn entry_for(path: &str) -> Result<&'static PluginEntry, String> {
    let reg = ENTRIES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut g = reg.lock().unwrap();
    if let Some(e) = g.get(path) { return Ok(*e); }
    // SAFETY: solo cargamos .clap que el usuario elige/escanea explícitamente.
    let e = unsafe { PluginEntry::load(path) }.map_err(|e| format!("no se pudo cargar: {}", e))?;
    let leaked: &'static PluginEntry = Box::leak(Box::new(e));
    g.insert(path.to_string(), leaked);
    Ok(leaked)
}

/// Directorios CLAP estándar en Linux.
fn default_dirs() -> Vec<std::path::PathBuf> {
    let mut v = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let base = std::path::PathBuf::from(home);
        v.push(base.join(".clap"));
        v.push(base.join(".local/lib/clap"));
    }
    v.push("/usr/lib/clap".into());
    v.push("/usr/local/lib/clap".into());
    v
}

/// Carpetas estándar de CLAP (para mostrarlas en Preferencias).
pub fn standard_dirs() -> Vec<std::path::PathBuf> { default_dirs() }

/// Carpetas donde se buscan plugins CLAP: las estándar (si no se desactivan) más las del usuario.
pub fn scan_dirs() -> Vec<std::path::PathBuf> {
    let c = lanzadera_core::config::get();
    let mut v = if c.use_default_plugin_paths { default_dirs() } else { Vec::new() };
    for p in &c.clap_paths {
        let pb = std::path::PathBuf::from(p);
        if !v.contains(&pb) { v.push(pb); }
    }
    v
}

fn walk_clap(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>, depth: usize) {
    if depth > 4 { return; }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() { walk_clap(&p, out, depth + 1); }
        else if p.extension().and_then(|x| x.to_str()) == Some("clap") { out.push(p); }
    }
}

/// Escanea los directorios estándar y lista todos los plugins (ruta, id, nombre).
/// Carga cada bundle una sola vez (queda cacheado para el activate posterior).
pub fn scan() -> Vec<ClapRef> {
    scan_kinds().into_iter().map(|(r, _)| r).collect()
}

/// Como `scan`, pero con el rol de cada plugin según sus `features` CLAP
/// ("instrument", "note-effect", "audio-effect"). Sin features reconocibles → Unknown.
pub fn scan_kinds() -> Vec<(ClapRef, lanzadera_core::PluginKind)> {
    use lanzadera_core::PluginKind;
    let mut files = Vec::new();
    for d in scan_dirs() { walk_clap(&d, &mut files, 0); }
    let mut out = Vec::new();
    for f in files.into_iter().take(64) {
        let Some(path) = f.to_str().map(|s| s.to_string()) else { continue };
        let Ok(entry) = entry_for(&path) else { continue };
        let Some(factory) = entry.get_plugin_factory() else { continue };
        for d in factory.plugin_descriptors() {
            let Some(id) = d.id().map(|i| i.to_string_lossy().into_owned()) else { continue };
            let name = d.name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| id.clone());
            let (mut inst, mut note_fx, mut audio_fx) = (false, false, false);
            for ft in d.features() {
                match ft.to_bytes() {
                    b"instrument" => inst = true,
                    b"note-effect" => note_fx = true,
                    b"audio-effect" => audio_fx = true,
                    _ => {}
                }
            }
            let kind = if inst { PluginKind::Instrument }
                else if note_fx { PluginKind::NoteFx }
                else if audio_fx { PluginKind::Fx }
                else { PluginKind::Unknown };
            out.push(((path.clone(), id, name), kind));
        }
    }
    out
}

/// Carga y activa un plugin. `want_id = None` → el primero del bundle.
pub fn load(path: &str, want_id: Option<&str>, sample_rate: u32, max_frames: u32, track: usize)
    -> Result<(Instance, Processor, String, Vec<PluginParamInfo>), String>
{
    let entry = entry_for(path)?;
    let factory = entry.get_plugin_factory().ok_or("el bundle no expone plugin factory")?;
    let desc = factory.plugin_descriptors()
        .find(|d| matches!((want_id, d.id()),
            (Some(w), Some(i)) if i.to_bytes() == w.as_bytes()))
        .or_else(|| factory.plugin_descriptors().next())
        .ok_or("el bundle no contiene plugins")?;
    let id = desc.id().ok_or("plugin sin id")?.to_owned();
    let name = desc.name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "CLAP".into());

    let host_info = HostInfo::new("lanzadera", "DavidLanas", "https://github.com/DavidLanas", "0.1.0")
        .map_err(|e| e.to_string())?;
    let mut instance = PluginInstance::<LanzaderaHost>::new(
        |_| LanzaderaShared { track }, |shared| LanzaderaMainThread::new(shared.track), entry, &id, &host_info,
    ).map_err(|e| format!("no se pudo instanciar: {}", e))?;

    let cfg = PluginAudioConfiguration {
        sample_rate: sample_rate as f64,
        min_frames_count: 1,
        max_frames_count: max_frames,
    };
    // Los parámetros se consultan ANTES de activar el procesador de audio.
    // El closure de activate() no entrega el handle del plugin: entrega los
    // handlers del host. En clack-host 0.2, el handle de main thread del plugin
    // en estado inactivo se obtiene con inactive_main_thread().
    //
    // API: clack-host / clack-extensions (MIT/Apache).
    let params = describe_params(&mut instance);

    let stopped = instance
        .activate(|_, _| (), cfg)
        .map_err(|e| format!("activate: {}", e))?;

    Ok((instance, stopped.into(), name, params))
}

/// Enumera los parámetros expuestos por el plugin usando la extensión params.
///
/// Se llama desde el hilo principal/UI, antes de activar el procesador de audio.
/// No se llama desde el callback RT.
///
/// API basada en clack-extensions (MIT/Apache). La idea de exponer parámetros
/// al host para control desde UI/controlador sigue la semántica de sesión/mixer
/// de MAGDA (GPL-3), sin copiar su código C++/JUCE.
/// Enumera los parametros expuestos por el plugin (extension params de CLAP).
///
/// Se llama desde el hilo principal, ANTES de activate(). El handle correcto
/// en clack-host 0.2 es `PluginInstance::plugin_handle()` (metodo seguro).
///
/// API verificada en docs.rs: clack-host / clack-extensions (MIT/Apache).
/// Nota: CLAP usa valores "plain" (min_value..max_value), no normalizados.
fn describe_params(instance: &mut Instance) -> Vec<PluginParamInfo> {
    let mut out = Vec::new();
    let main = instance.plugin_handle();

    let Some(pp) = main.get_extension::<PluginParams>() else {
        return out;
    };

    let count = pp.count(&main);
    let mut buffer = ParamInfoBuffer::new();

    // Tope de 256 para mantener la tabla RT fija.
    for i in 0..count.min(256) {
        let Some(info) = pp.get_info(&main, i, &mut buffer) else { continue };

        if info.flags.contains(ParamInfoFlags::IS_HIDDEN)
            || info.flags.contains(ParamInfoFlags::IS_READONLY)
        {
            continue;
        }

        let min = info.min_value;
        let max = info.max_value;
        let raw = pp.get_value(&main, info.id).unwrap_or(info.default_value);

        // Normalizamos solo para mostrar en la UI (slider 0..1).
        let norm = if max > min {
            ((raw - min) / (max - min)).clamp(0.0, 1.0) as f32
        } else {
            raw.clamp(0.0, 1.0) as f32
        };

        out.push(PluginParamInfo {
            id: info.id.get(),
            name: String::from_utf8_lossy(info.name).into_owned(),
            value: norm,
            min,
            max,
        });
    }

    out
}

/// Pide al plugin su estado (extensión CLAP `state`). Hilo principal.
pub fn save_state(inst: &mut Instance) -> Option<Vec<u8>> {
    use clack_extensions::state::PluginState;
    let main = inst.plugin_handle();
    let st = main.get_extension::<PluginState>()?;
    let mut buf: Vec<u8> = Vec::new();
    st.save(&main, &mut buf).ok()?;
    Some(buf)
}

/// Restaura un estado guardado con `save_state`. Hilo principal. true si el plugin lo aceptó.
pub fn load_state(inst: &mut Instance, bytes: &[u8]) -> bool {
    use clack_extensions::state::PluginState;
    let main = inst.plugin_handle();
    let Some(st) = main.get_extension::<PluginState>() else { return false };
    let mut reader = bytes;
    st.load(&main, &mut reader).is_ok()
}
