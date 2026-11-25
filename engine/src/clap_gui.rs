// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! GUI flotante de plugins CLAP (Fase G6a).
//!
//! TODO ocurre en el hilo principal (App::update). Nunca en el callback RT.
//! El handle se obtiene con `PluginInstance::plugin_handle()`, igual que hace
//! `describe_params` en clap_host.rs.
//!
//! API verificada en docs.rs (clack-extensions 0.2.0, MIT/Apache):
//!   GuiConfiguration { api_type: GuiApiType, is_floating: bool }
//!   GuiApiType::X11 / ::WAYLAND / ::WIN32 / ::COCOA  (constantes asociadas)
//!   PluginGui::{is_api_supported, create, suggest_title, show, hide, destroy}
//!
//! G6a NO llama a `set_transient`: es el unico metodo unsafe y el unico que
//! exige un Window del host. Sin el, la ventana del plugin es independiente
//! en vez de "siempre encima". Funcional, y sin dependencias nuevas.
//!
//! La conducta (plugin con UI propia -> ventana flotante; sin UI -> editor
//! generico de sliders) sigue el modelo documentado de SceneStack; concepto
//! reimplementado, no codigo copiado.
//!
//! # Embebida (X11) como alternativa
//! Muchos plugins CLAP de Linux (nih-plug, baseview...) NO implementan la
//! ventana flotante: solo la GUI embebida, que exige que el HOST cree una
//! ventana y se la pase con `set_parent`. Si la flotante no esta soportada,
//! `toggle` crea una ventana X11 propia (`xwin::XWindow`, patron de
//! vst3-host/window.rs, MIT) y embebe ahi la GUI del plugin.
//!
//! Este modulo tambien reparte la peticion "abrir/cerrar interfaz" para
//! plugins VST3 (la UI no distingue formato): ver `AudioHandle::poll_gui_requests`.

use std::sync::Mutex;

use lanzadera_core::MAX_TRACKS;

/// Máximo de huecos de plugin con GUI: 1 instrumento + 8 Note FX + 8 FX por pista.
/// Esquema de ids: 0..31 instrumento · nfx_gui_id(t,i) Note FX · fx_gui_id(t,i) FX.
pub const CHAIN_MAX: usize = 8;
pub const MAX_PLUGIN_SLOTS: usize = MAX_TRACKS * (1 + 2 * CHAIN_MAX);
pub fn nfx_gui_id(t: usize, i: usize) -> usize {
    MAX_TRACKS + (t * CHAIN_MAX + i).min(MAX_TRACKS * CHAIN_MAX - 1)
}
pub fn fx_gui_id(t: usize, i: usize) -> usize {
    MAX_TRACKS + MAX_TRACKS * CHAIN_MAX + (t * CHAIN_MAX + i).min(MAX_TRACKS * CHAIN_MAX - 1)
}
/// Pista propietaria de un id de plugin (instrumento, Note FX o FX).
pub fn gui_id_track(id: usize) -> usize {
    if id < MAX_TRACKS { return id; }
    let off = id - MAX_TRACKS;
    (off % (MAX_TRACKS * CHAIN_MAX)) / CHAIN_MAX
}

/// Pista cuya GUI hay que abrir/cerrar. La UI encola; el hilo principal consume.
pub static GUI_TOGGLE: Mutex<Option<usize>> = Mutex::new(None);

/// Ventanas cerradas por el usuario: (pista, fue_destruida). Las escribe el
/// handler del host; las consume el hilo principal para llamar a destroy.
pub static GUI_CLOSED: Mutex<Vec<(usize, bool)>> = Mutex::new(Vec::new());

#[derive(Clone, Copy)]
pub struct GuiState {
    /// El plugin de esa pista expone la extension GUI.
    pub avail: [bool; MAX_PLUGIN_SLOTS],
    /// La GUI de esa pista esta abierta ahora mismo.
    pub open: [bool; MAX_PLUGIN_SLOTS],
    /// La GUI de esta pista ya colgo una vez esta sesion: no reintentar.
    pub broken: [bool; MAX_PLUGIN_SLOTS],
}

pub static GUI_STATE: Mutex<GuiState> = Mutex::new(GuiState {
    avail: [false; MAX_PLUGIN_SLOTS],
    open: [false; MAX_PLUGIN_SLOTS],
    broken: [false; MAX_PLUGIN_SLOTS],
});

/// Mensaje para mostrar en la barra de estado de la UI (errores de GUI /
/// de carga de plugins). Antes solo iban a stderr y el usuario no los veia.
static NOTICE: Mutex<Option<String>> = Mutex::new(None);

/// Publica un aviso para la UI y lo imprime en consola.
pub fn notify(msg: impl Into<String>) {
    let m = msg.into();
    eprintln!("{}", m);
    if let Ok(mut n) = NOTICE.lock() { *n = Some(m); }
}

/// Consume el ultimo aviso, si hay.
pub fn take_notice() -> Option<String> {
    NOTICE.lock().ok().and_then(|mut n| n.take())
}

/// Peticiones de cambio de tamano del plugin: (pista, ancho, alto).
static RESIZES: Mutex<Vec<(usize, u32, u32)>> = Mutex::new(Vec::new());

/// El plugin pide otro tamano (puede llegar fuera del hilo principal).
pub fn note_resize(track: usize, w: u32, h: u32) {
    if let Ok(mut q) = RESIZES.try_lock() { q.push((track, w, h)); }
}

pub fn take_resizes() -> Vec<(usize, u32, u32)> {
    RESIZES.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
}

/// true si alguna GUI de plugin esta abierta. main.rs lo usa para pedir
/// repintado periodico: las GUI VST3 de Linux (VSTGUI) solo se dibujan si el
/// host les da cuerda (`service_run_loop`) en cada frame.
pub fn any_open() -> bool {
    GUI_STATE.lock().map(|s| s.open.iter().any(|&o| o)).unwrap_or(false)
}

/// Estado para la UI: (tiene_gui, esta_abierta).
pub fn state(track: usize) -> (bool, bool) {
    if track >= MAX_PLUGIN_SLOTS { return (false, false); }
    GUI_STATE.lock().map(|s| (s.avail[track], s.open[track])).unwrap_or((false, false))
}

/// Pide abrir/cerrar la GUI de una pista. Lo llama la UI.
pub fn request_toggle(track: usize) {
    if let Ok(mut g) = GUI_TOGGLE.lock() { *g = Some(track); }
}

/// true si hay una peticion de abrir/cerrar sin atender (para pedir repintado).
pub fn has_pending_request() -> bool {
    GUI_TOGGLE.lock().map(|g| g.is_some()).unwrap_or(false)
}

/// Consume la peticion pendiente, si la hay.
pub fn take_request() -> Option<usize> {
    GUI_TOGGLE.lock().ok().and_then(|mut g| g.take())
}

/// El plugin avisa de que su ventana se cerro. Puede llegar fuera del hilo
/// principal: try_lock y se descarta si esta ocupado (mejor eso que bloquear).
pub fn note_closed(track: usize, was_destroyed: bool) {
    if let Ok(mut q) = GUI_CLOSED.try_lock() {
        q.push((track, was_destroyed));
    }
}

/// Drena los avisos de cierre.
pub fn take_closed() -> Vec<(usize, bool)> {
    GUI_CLOSED.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
}

/// Acusa recibo del cierre. La doc de CLAP exige llamar a `destroy` cuando
/// `was_destroyed` es true. Hilo principal.
#[cfg(feature = "clap")]
pub fn ack_closed(instance: &mut crate::clap_host::Instance, track: usize, was_destroyed: bool) {
    use clack_extensions::gui::PluginGui;
    let main = instance.plugin_handle();
    if let Some(gui) = main.get_extension::<PluginGui>() {
        if was_destroyed {
            gui.destroy(&main);
        } else {
            let _ = gui.hide(&main);
        }
    }
    set_open(track, false);
    println!("[gui] pista {}: el usuario cerro la ventana", track + 1);
}

pub fn set_avail(track: usize, v: bool) {
    if let Ok(mut s) = GUI_STATE.lock() {
        if track < MAX_PLUGIN_SLOTS { s.avail[track] = v; }
    }
}

pub fn set_open(track: usize, v: bool) {
    if let Ok(mut s) = GUI_STATE.lock() {
        if track < MAX_PLUGIN_SLOTS { s.open[track] = v; }
    }
}
pub fn set_broken(track: usize, v: bool) {
    if let Ok(mut s) = GUI_STATE.lock() {
        if track < MAX_PLUGIN_SLOTS { s.broken[track] = v; }
    }
}
pub fn is_broken(track: usize) -> bool {
    GUI_STATE.lock().map(|s| s.broken[track]).unwrap_or(false)
}

/// API de ventana preferida para esta plataforma.
#[cfg(feature = "clap")]
fn platform_api() -> clack_extensions::gui::GuiApiType<'static> {
    use clack_extensions::gui::GuiApiType;
    if cfg!(target_os = "windows") { GuiApiType::WIN32 }
    else if cfg!(target_os = "macos") { GuiApiType::COCOA }
    else { GuiApiType::X11 }
}

/// Comprueba al cargar si el plugin expone GUI, para poder atenuar el boton.
#[cfg(feature = "clap")]
pub fn probe(instance: &mut crate::clap_host::Instance, track: usize) {
    let main = instance.plugin_handle();
    let has = main.get_extension::<clack_extensions::gui::PluginGui>().is_some();
    set_avail(track, has);
    set_open(track, false);
}

/// Ventanas X11 anfitrionas de las GUI CLAP embebidas, una por pista.
/// Vive en `AudioHandle` (hilo principal).
#[cfg(feature = "clap")]
#[derive(Default)]
pub struct EmbedHost {
    wins: Vec<(usize, crate::xwin::XWindow)>,
}

#[cfg(feature = "clap")]
impl EmbedHost {
    fn remove(&mut self, track: usize) { self.wins.retain(|(t, _)| *t != track); }
}

/// Guard de llamadas al plugin: SIGALRM si el plugin bloquea > TIMEOUT_SECS
/// y catch_unwind para panics de Rust. No protege de SIGSEGV (eso requiere
/// proceso separado, ver LANZADERA_VST3_ISOLATE), pero sí de deadlocks y panics.
/// (Portado del análisis de LYNX-SHARE PluginSandbox; aquí alarm() de POSIX.)
#[cfg(all(feature = "clap", target_os = "linux"))]
mod crash_guard {
    use std::sync::atomic::{AtomicBool, Ordering};
    /// Segundos máximos para gui.create()/gui.show().
    const TIMEOUT_SECS: libc::c_uint = 6;
    #[allow(dead_code)]
    pub(super) static IN_PLUGIN_GUI_CALL: AtomicBool = AtomicBool::new(false);
    /// true si el último watchdog disparó (la llamada excedió TIMEOUT_SECS).
    static ALARM_FIRED: AtomicBool = AtomicBool::new(false);
    /// Consume el flag de watchdog: true si la ultima llamada vigilada
    /// excedio TIMEOUT_SECS (aunque finalmente retornara).
    pub fn alarm_fired() -> bool { ALARM_FIRED.swap(false, Ordering::AcqRel) }
    /// Handler vacío: solo interrumpe la syscall bloqueada del plugin (EINTR).
    pub fn install_alarm_handler() {
        use std::sync::Once;
        static INSTALLED: Once = Once::new();
        INSTALLED.call_once(|| {
            unsafe extern "C" fn alarm_noop(_sig: libc::c_int) {
            ALARM_FIRED.store(true, Ordering::Release);
        }
            unsafe { libc::signal(libc::SIGALRM, alarm_noop as *const () as libc::sighandler_t); }
        });
    }
    pub fn guarded<F, R>(f: F) -> Result<R, String>
    where F: FnOnce() -> Result<R, String> + std::panic::UnwindSafe {
        IN_PLUGIN_GUI_CALL.store(true, Ordering::Release);
        unsafe { libc::alarm(TIMEOUT_SECS); }
        let result = std::panic::catch_unwind(f);
        unsafe { libc::alarm(0); }
        IN_PLUGIN_GUI_CALL.store(false, Ordering::Release);
        match result {
            Ok(r) => r,
            Err(_) => Err("el plugin sufrió un panic al abrir la GUI                  (bug del plugin; reinicia Lanzadera si hay audio corrupto)".to_string()),
        }
    }
}

/// Instala el watchdog SIGALRM para GUI de plugins CLAP. No-op fuera de Linux/clap.
pub fn install_alarm_handler() {
    #[cfg(all(feature = "clap", target_os = "linux"))]
    crash_guard::install_alarm_handler();
}

/// Vigila una llamada de GUI en Linux (watchdog + catch_unwind); en otras
/// plataformas llama directamente.
#[cfg(feature = "clap")]
fn guard_gui<F>(f: F) -> Result<(), String>
where F: FnOnce() -> Result<(), String> + std::panic::UnwindSafe {
    #[cfg(target_os = "linux")]
    { crash_guard::guarded(f) }
    #[cfg(not(target_os = "linux"))]
    { f() }
}

/// true si el watchdog disparó durante la última llamada vigilada.
#[cfg(feature = "clap")]
fn gui_timed_out() -> bool {
    #[cfg(target_os = "linux")]
    { crash_guard::alarm_fired() }
    #[cfg(not(target_os = "linux"))]
    { false }
}

/// Abre o cierra la GUI de una pista (flotante si el plugin la ofrece; si no,
/// embebida en una ventana X11 del host). Hilo principal.
#[cfg(feature = "clap")]
pub fn toggle(instance: &mut crate::clap_host::Instance, track: usize, title: &str,
              embed: &mut EmbedHost) -> Result<(), String>
{
    use clack_extensions::gui::{GuiApiType, GuiConfiguration, GuiSize, PluginGui, Window};

    let main = instance.plugin_handle();
    let Some(gui) = main.get_extension::<PluginGui>() else {
        return Err("el plugin no expone interfaz grafica".into());
    };

    if state(track).1 {
        let _ = guard_gui(std::panic::AssertUnwindSafe(|| {
            let _ = gui.hide(&main);
            gui.destroy(&main);
            Ok(())
        }));
        embed.remove(track); // la ventana host se destruye DESPUES de destroy()
        set_open(track, false);
        return Ok(());
    }

    // Si este plugin ya colgó la GUI esta sesión, no reintentar: congelaría
    // otra vez el hilo principal. Sugerir VST3 o reinicio.
    if is_broken(track) {
        return Err("la GUI de este plugin ya colgó esta sesión (marcada como                     insegura). Usa su versión VST3 —las ventanas funcionan— o                     reinicia Lanzadera para reintentar.".into());
    }
    // Negociacion, como pide la secuencia documentada de CLAP: primero se
    // consulta la preferencia del plugin, luego se comprueba el soporte.
    let pref = gui.get_preferred_api(&main);
    if let Some(p) = pref {
        println!("[gui] pista {}: prefiere {:?} flotante={}", track + 1, p.api_type, p.is_floating);
    }
    let api = platform_api();
    let floating = GuiConfiguration { api_type: api, is_floating: true };
    let embedded = GuiConfiguration { api_type: api, is_floating: false };
    let f_ok = gui.is_api_supported(&main, floating);
    let e_ok = gui.is_api_supported(&main, embedded);
    println!("[gui] pista {}: {:?} flotante={} embebido={}", track + 1, api, f_ok, e_ok);

    // 1) Flotante en la API de la plataforma, o la flotante que prefiera el plugin.
    let mut cfg = floating;
    let mut use_float = f_ok;
    if !use_float {
        if let Some(p) = pref {
            if p.is_floating && gui.is_api_supported(&main, p) { cfg = p; use_float = true; }
        }
    }
    if use_float {
        #[cfg(target_os = "linux")]
        let result = crash_guard::guarded(std::panic::AssertUnwindSafe(|| {
            gui.create(&main, cfg).map_err(|e| format!("create: {:?}", e))?;
            let ctitle = std::ffi::CString::new(title)
                .unwrap_or_else(|_| std::ffi::CString::new("plugin").unwrap());
            gui.suggest_title(&main, &ctitle);
            gui.show(&main).map_err(|e| format!("show: {:?}", e))
        }));
        #[cfg(not(target_os = "linux"))]
        let result = {
            gui.create(&main, cfg).map_err(|e| format!("create: {:?}", e))?;
            let ctitle = std::ffi::CString::new(title)
                .unwrap_or_else(|_| std::ffi::CString::new("plugin").unwrap());
            gui.suggest_title(&main, &ctitle);
            gui.show(&main).map_err(|e| format!("show: {:?}", e))
        };
        match result {
            Ok(()) => { set_open(track, true); return Ok(()); }
            Err(e) => {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gui.destroy(&main)));
                return Err(e);
            }
        }
    }

    // 2) Embebida: el host aporta la ventana (solo X11 en Linux).
    if !e_ok || api != GuiApiType::X11 {
        return Err("el plugin no ofrece GUI flotante ni embebida en esta plataforma".into());
    }
    #[cfg(target_os = "linux")]
    let create_result = crash_guard::guarded(std::panic::AssertUnwindSafe(|| {
        gui.create(&main, embedded).map_err(|e| format!("create(embebida): {:?}", e))
    }));
    #[cfg(not(target_os = "linux"))]
    let create_result = gui.create(&main, embedded).map_err(|e| format!("create(embebida): {:?}", e));
    if let Err(e) = create_result {
        if gui_timed_out() { set_broken(track, true); }
        return Err(e);
    }
    // Escala 1.0: sin esto algunos plugins (JUCE) asumen HiDPI inexistente.
    let _ = gui.set_scale(&main, 1.0);
    let size = gui.get_size(&main).unwrap_or(GuiSize { width: 800, height: 600 });
    let win = match crate::xwin::XWindow::new(title, size.width, size.height) {
        Ok(w) => w,
        Err(e) => { gui.destroy(&main); return Err(e); }
    };
    // SAFETY: la ventana vive en `embed` hasta que se llama a destroy() arriba
    // o en close_all/poll_embedded, siempre en ese orden.
    let parent_res = guard_gui(std::panic::AssertUnwindSafe(|| {
        unsafe { gui.set_parent(&main, Window::from_x11_handle(win.id() as std::ffi::c_ulong)) }
            .map_err(|e| format!("set_parent: {:?}", e))?;
        // En GUI embebida `show` es opcional: un fallo NO es fatal.
        if let Err(e) = gui.show(&main) {
            println!("[gui] pista {}: show() embebida devolvio {:?} (se ignora)", track + 1, e);
        }
        Ok(())
    }));
    if let Err(e) = parent_res {
        if gui_timed_out() { set_broken(track, true); }
        let _ = guard_gui(std::panic::AssertUnwindSafe(|| { gui.destroy(&main); Ok(()) }));
        embed.remove(track);
        return Err(e);
    }
    embed.wins.push((track, win));
    set_open(track, true);
    println!("[gui] pista {}: GUI embebida en ventana X11 del host", track + 1);
    Ok(())
}

/// Atiende cierres por boton de ventana y peticiones de resize de las GUI
/// embebidas. Llamar cada frame desde el hilo principal.
#[cfg(feature = "clap")]
pub fn poll_embedded(instances: &mut [(usize, crate::clap_host::Instance)], embed: &mut EmbedHost) {
    use clack_extensions::gui::{GuiSize, PluginGui};

    // Resize pedido por el plugin: primero la ventana, luego set_size.
    for (t, w, h) in take_resizes() {
        let Some((_, win)) = embed.wins.iter().find(|(i, _)| *i == t) else { continue };
        win.resize(w, h);
        if let Some((_, inst)) = instances.iter_mut().find(|(i, _)| *i == t) {
            let main = inst.plugin_handle();
            if let Some(gui) = main.get_extension::<PluginGui>() {
                let _ = gui.set_size(&main, GuiSize { width: w, height: h });
            }
        }
    }

    // Cierre por la X de la ventana.
    let closed: Vec<usize> = embed.wins.iter()
        .filter(|(_, w)| w.poll_close()).map(|(t, _)| *t).collect();
    for t in closed {
        if let Some((_, inst)) = instances.iter_mut().find(|(i, _)| *i == t) {
            let main = inst.plugin_handle();
            if let Some(gui) = main.get_extension::<PluginGui>() {
                let _ = gui.hide(&main);
                gui.destroy(&main);
            }
        }
        embed.remove(t);
        set_open(t, false);
        println!("[gui] pista {}: el usuario cerro la ventana", t + 1);
    }
}

/// Cierra todas las GUI abiertas. Se llama antes de soltar las instancias.
#[cfg(feature = "clap")]
pub fn close_all(instances: &mut [(usize, crate::clap_host::Instance)], embed: &mut EmbedHost) {
    use clack_extensions::gui::PluginGui;
    for (t, inst) in instances.iter_mut() {
        if !state(*t).1 { continue; }
        let main = inst.plugin_handle();
        if let Some(gui) = main.get_extension::<PluginGui>() {
            let _ = gui.hide(&main);
            gui.destroy(&main);
        }
        set_open(*t, false);
    }
    embed.wins.clear(); // despues de destroy(): la ventana padre sobrevive al plugin
}
