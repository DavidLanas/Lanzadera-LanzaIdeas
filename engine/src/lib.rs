//! lanzadera-engine — estado compartido, motor de clips, audio I/O, loader,
//! MIDI, sinte interno y EQ (Fase 3A).
pub mod shared;
pub mod clip_engine;
pub mod rt;
pub mod loader;
pub mod audio_io;
pub mod audio_input;
pub mod audio_record;
pub mod midi;
pub mod synth;
pub mod eq;
pub mod rec;
#[cfg(feature = "clap")]
pub mod clap_host;
#[cfg(feature = "clap")]
pub mod clap_params;

/// true si el binario se compiló con hosting CLAP (--features clap).
pub const CLAP_ENABLED: bool = cfg!(feature = "clap");

/// Escaneo de plugins CLAP accesible desde la GUI sin importar la feature.
/// Devuelve (ruta, id, nombre). Vacío si el binario no tiene --features clap.
#[cfg(feature = "clap")]
pub fn clap_scan() -> Vec<clap_host::ClapRef> { clap_host::scan() }
#[cfg(not(feature = "clap"))]
pub fn clap_scan() -> Vec<(String, String, String)> { Vec::new() }

pub use shared::SessionShared;
pub use clip_engine::{ClipEngine, ClipEvent};
pub use rt::RtStore;

pub mod caja_drums;
pub mod groove;
pub mod groove_kit;
