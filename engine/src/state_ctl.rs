//! Coordinación UI ↔ hilo principal para el estado de los plugins.
//! Las instancias de plugin viven en `AudioHandle`, que posee el hilo principal de la
//! app; la UI (que guarda proyectos) solo puede pedir la captura y esperar a que se haga.
use std::sync::atomic::{AtomicBool, Ordering};

static CAPTURE_REQ: AtomicBool = AtomicBool::new(false);
static SKIP_CAPTURE: AtomicBool = AtomicBool::new(false);
static PLUGIN_DIRTY: AtomicBool = AtomicBool::new(false);

/// La UI pide que se guarde el estado de todos los plugins en la sesión.
pub fn request_capture() { CAPTURE_REQ.store(true, Ordering::Relaxed); }
/// El hilo principal consume la petición (devuelve true si había una).
pub fn take_capture_request() -> bool { CAPTURE_REQ.swap(false, Ordering::Relaxed) }
/// ¿Hay una captura pedida que aún no se ha hecho?
pub fn capture_pending() -> bool { CAPTURE_REQ.load(Ordering::Relaxed) }

/// Al abrir/crear un proyecto el audio se reinicia, pero NO debe capturarse el estado
/// de los plugins viejos (pisaría el estado del proyecto recién cargado).
pub fn skip_capture_on_next_restart() { SKIP_CAPTURE.store(true, Ordering::Relaxed); }
pub fn take_skip_capture() -> bool { SKIP_CAPTURE.swap(false, Ordering::Relaxed) }

/// Un plugin CLAP avisó (`state.mark_dirty`) de que su estado cambió.
pub fn mark_plugin_dirty() { PLUGIN_DIRTY.store(true, Ordering::Relaxed); }
pub fn take_plugin_dirty() -> bool { PLUGIN_DIRTY.swap(false, Ordering::Relaxed) }
