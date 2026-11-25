// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! Fase F5a: tabla de parametros CLAP para control desde UI/controlador.
//!
//! El hilo de audio no puede hacer locks ni asignaciones. Guardamos en tablas
//! fijas de atomos, indexadas por (track, param_idx):
//!   - el `param_id` real de CLAP
//!   - min_value / max_value (plain), para desnormalizar el 0..1 de la UI
//!
//! CLAP trabaja con valores "plain", no normalizados: hay que convertir
//! `plain = min + norm * (max - min)` antes de enviar ParamValueEvent.
//!
//! API verificada en docs.rs: clack-extensions::params (MIT/Apache).
//! Referencia conceptual de control de parametros desde el host: MAGDA (GPL-3).
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;

use lanzadera_core::MAX_TRACKS;

use crate::clap_host::PluginParamInfo;

pub const MAX_PLUGIN_PARAMS: usize = 256;
const SLOTS: usize = MAX_TRACKS * MAX_PLUGIN_PARAMS;

static PARAM_IDS: OnceLock<Box<[AtomicU32]>> = OnceLock::new();
static PARAM_MIN: OnceLock<Box<[AtomicU32]>> = OnceLock::new();
static PARAM_MAX: OnceLock<Box<[AtomicU32]>> = OnceLock::new();

fn table(lock: &OnceLock<Box<[AtomicU32]>>) -> &[AtomicU32] {
    lock.get_or_init(|| (0..SLOTS).map(|_| AtomicU32::new(0)).collect())
}

/// Publica la tabla de una pista. Solo desde el hilo principal, nunca RT.
pub fn set_track_params(track: usize, params: &[PluginParamInfo]) {
    if track >= MAX_TRACKS { return; }
    let ids = table(&PARAM_IDS);
    let mins = table(&PARAM_MIN);
    let maxs = table(&PARAM_MAX);
    let base = track * MAX_PLUGIN_PARAMS;
    let n = params.len().min(MAX_PLUGIN_PARAMS);

    for (i, p) in params.iter().take(n).enumerate() {
        ids[base + i].store(p.id, Ordering::Relaxed);
        mins[base + i].store((p.min as f32).to_bits(), Ordering::Relaxed);
        maxs[base + i].store((p.max as f32).to_bits(), Ordering::Release);
    }
    for i in n..MAX_PLUGIN_PARAMS {
        ids[base + i].store(0, Ordering::Release);
    }
}

pub fn clear_track_params(track: usize) {
    set_track_params(track, &[]);
}

/// Traduce (track, param_idx, valor 0..1 de la UI) a (clap_param_id, valor plain).
/// Solo loads atomicos: seguro en el callback RT, sin alloc ni locks.
pub fn param_plain(track: usize, param_idx: usize, norm: f32) -> Option<(u32, f64)> {
    if track >= MAX_TRACKS || param_idx >= MAX_PLUGIN_PARAMS { return None; }
    let i = track * MAX_PLUGIN_PARAMS + param_idx;
    let id = table(&PARAM_IDS)[i].load(Ordering::Acquire);
    if id == 0 { return None; }
    let min = f32::from_bits(table(&PARAM_MIN)[i].load(Ordering::Relaxed)) as f64;
    let max = f32::from_bits(table(&PARAM_MAX)[i].load(Ordering::Relaxed)) as f64;
    let v = norm.clamp(0.0, 1.0) as f64;
    let plain = if max > min { min + v * (max - min) } else { v };
    Some((id, plain))
}

/// Compatibilidad con llamadas antiguas.
pub fn read_params() -> Vec<(String, String, f32)> {
    Vec::new()
}
