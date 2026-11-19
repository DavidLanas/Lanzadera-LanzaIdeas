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

use std::sync::Mutex;

use lanzadera_core::MAX_TRACKS;

/// Pista cuya GUI hay que abrir/cerrar. La UI encola; el hilo principal consume.
pub static GUI_TOGGLE: Mutex<Option<usize>> = Mutex::new(None);

/// Ventanas cerradas por el usuario: (pista, fue_destruida). Las escribe el
/// handler del host; las consume el hilo principal para llamar a destroy.
pub static GUI_CLOSED: Mutex<Vec<(usize, bool)>> = Mutex::new(Vec::new());

#[derive(Clone, Copy)]
pub struct GuiState {
    /// El plugin de esa pista expone la extension GUI.
    pub avail: [bool; MAX_TRACKS],
    /// La GUI de esa pista esta abierta ahora mismo.
    pub open: [bool; MAX_TRACKS],
}

pub static GUI_STATE: Mutex<GuiState> = Mutex::new(GuiState {
    avail: [false; MAX_TRACKS],
    open: [false; MAX_TRACKS],
});

/// Estado para la UI: (tiene_gui, esta_abierta).
pub fn state(track: usize) -> (bool, bool) {
    if track >= MAX_TRACKS { return (false, false); }
    GUI_STATE.lock().map(|s| (s.avail[track], s.open[track])).unwrap_or((false, false))
}

/// Pide abrir/cerrar la GUI de una pista. Lo llama la UI.
pub fn request_toggle(track: usize) {
    if let Ok(mut g) = GUI_TOGGLE.lock() { *g = Some(track); }
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

#[cfg(feature = "clap")]
fn set_avail(track: usize, v: bool) {
    if let Ok(mut s) = GUI_STATE.lock() {
        if track < MAX_TRACKS { s.avail[track] = v; }
    }
}

#[cfg(feature = "clap")]
fn set_open(track: usize, v: bool) {
    if let Ok(mut s) = GUI_STATE.lock() {
        if track < MAX_TRACKS { s.open[track] = v; }
    }
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

/// Abre o cierra la GUI flotante de una pista. Hilo principal.
#[cfg(feature = "clap")]
pub fn toggle(instance: &mut crate::clap_host::Instance, track: usize, title: &str)
    -> Result<(), String>
{
    use clack_extensions::gui::{GuiConfiguration, PluginGui};

    let main = instance.plugin_handle();
    let Some(gui) = main.get_extension::<PluginGui>() else {
        return Err("el plugin no expone interfaz grafica".into());
    };

    let was_open = state(track).1;
    if was_open {
        let _ = gui.hide(&main);
        gui.destroy(&main);
        set_open(track, false);
        return Ok(());
    }

    // Negociacion, como pide la secuencia documentada de CLAP: primero se
    // consulta la preferencia del plugin, luego se comprueba el soporte.
    let pref = gui.get_preferred_api(&main);
    if let Some(p) = pref {
        println!("[gui] pista {}: prefiere {:?} flotante={}", track + 1, p.api_type, p.is_floating);
    }
    let api = platform_api();
    for (name, a) in [("x11", clack_extensions::gui::GuiApiType::X11),
                      ("wayland", clack_extensions::gui::GuiApiType::WAYLAND)] {
        let f = gui.is_api_supported(&main, GuiConfiguration { api_type: a, is_floating: true });
        let e = gui.is_api_supported(&main, GuiConfiguration { api_type: a, is_floating: false });
        println!("[gui] pista {}: {} flotante={} embebido={}", track + 1, name, f, e);
    }

    // Preferimos flotante en la API de la plataforma; si el plugin prefiere
    // otra cosa flotante, se respeta.
    let mut cfg = GuiConfiguration { api_type: api, is_floating: true };
    if !gui.is_api_supported(&main, cfg) {
        if let Some(p) = pref {
            if p.is_floating && gui.is_api_supported(&main, p) {
                cfg = p;
            }
        }
    }
    if !gui.is_api_supported(&main, cfg) {
        return Err(format!("el plugin no soporta ventana flotante {:?}", cfg.api_type));
    }
    gui.create(&main, cfg).map_err(|e| format!("create: {:?}", e))?;

    let ctitle = std::ffi::CString::new(title).unwrap_or_else(|_| std::ffi::CString::new("plugin").unwrap());
    gui.suggest_title(&main, &ctitle);

    if let Err(e) = gui.show(&main) {
        gui.destroy(&main);
        return Err(format!("show: {:?}", e));
    }
    set_open(track, true);
    Ok(())
}

/// Cierra todas las GUI abiertas. Se llama antes de soltar las instancias.
#[cfg(feature = "clap")]
pub fn close_all(instances: &mut [(usize, crate::clap_host::Instance)]) {
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
}
