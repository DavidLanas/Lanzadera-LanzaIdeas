//! Pista del launcher: clips + canal de mezcla (estilo consola Vibez).
use serde::{Deserialize, Serialize};
use crate::{Clip, MAX_SCENES};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrackKind { Instrument, Audio, Return, Master }

/// Rol de un plugin según lo que declara (CLAP `features`, VST3 `sub_categories`).
/// `Unknown` = no se pudo saber sin cargarlo; el browser lo muestra en todas las listas.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginKind { Instrument, NoteFx, Fx, #[default] Unknown }

impl PluginKind {
    pub fn label(self) -> &'static str {
        match self {
            PluginKind::Instrument => "Instrumento",
            PluginKind::NoteFx => "Note FX",
            PluginKind::Fx => "FX",
            PluginKind::Unknown => "?",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Track {
    pub kind: TrackKind,
    pub name: String,
    pub color: [u8; 3],
    // serde soporta arrays hasta 32 elementos: MAX_SCENES = 32 (si sube, migrar a Vec).
    pub clips: [Option<Clip>; MAX_SCENES],
    pub volume: f32, // 0.0–1.0 lineal
    pub pan: f32,    // -1.0..+1.0
    pub mute: bool,
    pub solo: bool,
    pub arm: bool,
    pub eq_low: f32,  // -15..+15 dB
    pub eq_mid: f32,
    pub eq_high: f32,
    /// Frecuencia central del mid (Hz). serde(default) = proyectos viejos.
    #[serde(default = "def_eq_mid_f")]
    pub eq_mid_f: f32,
    /// Q de la campana del mid (ProQ-style). Más alto = más estrecho.
    #[serde(default = "def_eq_mid_q")]
    pub eq_mid_q: f32,
    pub hpf_on: bool,
    pub lpf_on: bool,
    /// Cortes del EQ gráfico (Fase 3). serde(default) = proyectos viejos siguen cargando.
    #[serde(default = "def_hpf_hz")]
    pub hpf_hz: f32,
    #[serde(default = "def_lpf_hz")]
    pub lpf_hz: f32,
    pub sends: [f32; 4],
    /// Nombre del dispositivo de entrada de audio (None = default del sistema).
    /// Usado al armar la pista para grabar loops de instrumentos analógicos.
    #[serde(default)]
    pub input_device: Option<String>,
    /// Ruta del plugin CLAP de la pista (Fase 3B). None = sinte interno.
    #[serde(default)]
    pub plugin_path: Option<String>,
    /// ID concreto del plugin dentro del bundle .clap (Fase 3E).
    #[serde(default)]
    pub plugin_id: Option<String>,
    /// Parámetros del plugin CLAP: (id, nombre, valor 0.0-1.0).
    /// Se actualizan al cargar el plugin y al cambiar valores desde el KeyLab.
    #[serde(default)]
    pub plugin_params: Vec<(String, String, f32)>,
    /// Note FX de la pista (plugin que transforma notas antes del instrumento).
    /// Paso 1: se elige y se guarda en el proyecto; el procesado llega en el paso 2.
    #[serde(default)]
    pub note_fx_path: Option<String>,
    #[serde(default)]
    pub note_fx_id: Option<String>,
    /// FX de audio de la pista (insert después del instrumento / entrada).
    #[serde(default)]
    pub fx_path: Option<String>,
    #[serde(default)]
    pub fx_id: Option<String>,
    /// Estado interno de cada plugin (base64): parámetros, presets, lo que el plugin guarde.
    /// Se captura al guardar el proyecto y antes de reiniciar el audio.
    #[serde(default)]
    pub plugin_state: Option<String>,
    #[serde(default)]
    pub note_fx_state: Option<String>,
    #[serde(default)]
    pub fx_state: Option<String>,
    /// Suma L+R en ambos lados (para una entrada mono grabada en un solo canal).
    #[serde(default)]
    pub mono: bool,
}

fn def_hpf_hz() -> f32 { 20.0 }
fn def_lpf_hz() -> f32 { 20000.0 }
fn def_eq_mid_f() -> f32 { 1000.0 }
fn def_eq_mid_q() -> f32 { 0.7 }

impl Default for Track {
    fn default() -> Self {
        Track {
            kind: TrackKind::Instrument,
            name: String::new(),
            color: [90, 160, 220],
            clips: std::array::from_fn(|_| None),
            volume: 0.8,
            pan: 0.0,
            mute: false,
            solo: false,
            arm: false,
            eq_low: 0.0,
            eq_mid: 0.0,
            eq_high: 0.0,
            eq_mid_f: 1000.0,
            eq_mid_q: 0.7,
            hpf_on: false,
            lpf_on: false,
            hpf_hz: 20.0,
            lpf_hz: 20000.0,
            sends: [0.0; 4],
            input_device: None,
            plugin_path: None,
            plugin_id: None,
            plugin_params: Vec::new(),
            note_fx_path: None,
            note_fx_id: None,
            fx_path: None,
            fx_id: None,
            plugin_state: None,
            note_fx_state: None,
            fx_state: None,
            mono: false,
        }
    }
}
