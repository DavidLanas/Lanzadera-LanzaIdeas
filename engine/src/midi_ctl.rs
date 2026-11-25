// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! Peticiones y estado de los controladores MIDI entre la UI (Preferencias) y el hilo
//! principal de la app, que es quien posee las conexiones.
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiProfile { KeyLab, Mk3, Generic }

impl MidiProfile {
    pub fn id(self) -> &'static str {
        match self { MidiProfile::KeyLab => "keylab", MidiProfile::Mk3 => "mk3", MidiProfile::Generic => "generic" }
    }
    pub fn from_id(s: &str) -> Option<Self> {
        match s { "keylab" => Some(MidiProfile::KeyLab), "mk3" => Some(MidiProfile::Mk3), "generic" => Some(MidiProfile::Generic), _ => None }
    }
    pub fn label(self) -> &'static str {
        match self {
            MidiProfile::KeyLab => "KeyLab Essential 49",
            MidiProfile::Mk3 => "Maschine Mikro MK3",
            MidiProfile::Generic => "Teclado / controlador",
        }
    }
}

#[derive(Clone, Debug)]
pub enum MidiRequest {
    /// Vuelve a listar los puertos (y reconecta los marcados como automáticos).
    Rescan,
    Connect { profile: MidiProfile, port: String },
    Disconnect { profile: MidiProfile, port: String },
}

#[derive(Clone, Debug, Default)]
pub struct MidiStatus {
    /// Puertos de entrada detectados.
    pub ports: Vec<String>,
    /// (perfil, puerto) de las conexiones activas.
    pub connected: Vec<(MidiProfile, String)>,
    /// Último mensaje (conectado / error).
    pub msg: String,
}

fn reqs() -> &'static Mutex<Vec<MidiRequest>> {
    static R: OnceLock<Mutex<Vec<MidiRequest>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Vec::new()))
}

fn stat() -> &'static Mutex<MidiStatus> {
    static S: OnceLock<Mutex<MidiStatus>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(MidiStatus::default()))
}

/// La UI pide algo; la app lo atiende en su siguiente frame.
pub fn request(r: MidiRequest) { if let Ok(mut v) = reqs().lock() { v.push(r); } }
pub fn take_requests() -> Vec<MidiRequest> {
    reqs().lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default()
}
pub fn status() -> MidiStatus { stat().lock().map(|s| s.clone()).unwrap_or_default() }
pub fn set_status(s: MidiStatus) { if let Ok(mut g) = stat().lock() { *g = s; } }
