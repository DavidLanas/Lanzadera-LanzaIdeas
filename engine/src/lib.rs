// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! lanzadera-engine — estado compartido, motor de clips, audio I/O, loader,
//! MIDI, sinte interno y EQ (Fase 3A).
pub mod shared;
pub mod midi_ctl;
pub mod state_ctl;
pub mod clip_engine;
pub mod rt;
pub mod loader;
pub mod midi_file;
pub mod plugin_events;
pub mod clap_gui;
pub mod audio_io;
pub mod audio_input;
pub mod audio_record;
pub mod midi;
pub mod synth;
pub mod eq;
pub mod rec;
#[cfg(feature = "clap")]
pub mod xwin;
#[cfg(feature = "clap")]
pub mod clap_host;
#[cfg(feature = "clap")]
pub mod clap_params;
#[cfg(feature = "clap")]
pub mod clap_services;

/// true si el binario se compiló con hosting CLAP (--features clap).
pub const CLAP_ENABLED: bool = cfg!(feature = "clap");
/// true si el binario se compiló con hosting VST3 (--features vst3).
pub const VST3_ENABLED: bool = cfg!(feature = "vst3");
#[cfg(feature = "vst3")]
pub mod vst3_slot;
#[cfg(feature = "vst3")]
pub fn vst3_scan() -> Vec<vst3_slot::Vst3Ref> { vst3_slot::scan() }
#[cfg(not(feature = "vst3"))]
pub fn vst3_scan() -> Vec<(String, String)> { Vec::new() }
/// Igual que `vst3_scan`, con el rol de cada plugin (Instrumento / Note FX / FX).
#[cfg(feature = "vst3")]
pub fn vst3_scan_kinds() -> Vec<(vst3_slot::Vst3Ref, lanzadera_core::PluginKind)> { vst3_slot::scan_kinds() }
#[cfg(not(feature = "vst3"))]
pub fn vst3_scan_kinds() -> Vec<((String, String), lanzadera_core::PluginKind)> { Vec::new() }

/// Escaneo de plugins CLAP accesible desde la GUI sin importar la feature.
/// Devuelve (ruta, id, nombre). Vacío si el binario no tiene --features clap.
#[cfg(feature = "clap")]
pub fn clap_scan() -> Vec<clap_host::ClapRef> { clap_host::scan() }
#[cfg(not(feature = "clap"))]
pub fn clap_scan() -> Vec<(String, String, String)> { Vec::new() }
/// Carpetas estándar donde se buscan plugins (Preferencias las muestra).
#[cfg(feature = "clap")]
pub fn clap_standard_dirs() -> Vec<std::path::PathBuf> { clap_host::standard_dirs() }
#[cfg(not(feature = "clap"))]
pub fn clap_standard_dirs() -> Vec<std::path::PathBuf> { Vec::new() }
#[cfg(feature = "vst3")]
pub fn vst3_standard_dirs() -> Vec<std::path::PathBuf> { vst3_slot::standard_dirs() }
#[cfg(not(feature = "vst3"))]
pub fn vst3_standard_dirs() -> Vec<std::path::PathBuf> { Vec::new() }

/// Igual que `clap_scan`, con el rol de cada plugin (Instrumento / Note FX / FX).
#[cfg(feature = "clap")]
pub fn clap_scan_kinds() -> Vec<(clap_host::ClapRef, lanzadera_core::PluginKind)> { clap_host::scan_kinds() }
#[cfg(not(feature = "clap"))]
pub fn clap_scan_kinds() -> Vec<((String, String, String), lanzadera_core::PluginKind)> { Vec::new() }

pub use shared::SessionShared;
pub use clip_engine::{ClipEngine, ClipEvent};
pub use rt::RtStore;

pub mod caja_drums;
pub mod groove;
pub mod groove_kit;
pub mod sample_edit;
