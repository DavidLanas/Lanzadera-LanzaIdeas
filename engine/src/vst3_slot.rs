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

use std::path::PathBuf;
use vst3_host::{
    audio::BusAudioBuffers,
    midi::{MidiChannel, MidiEvent},
    plugin::Plugin,
    simple,
};

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
pub struct Vst3Slot {
    pub plugin: Plugin,
    pub buffers: BusAudioBuffers,  // reutilizar en cada frame
    pub params: Vec<Vst3ParamInfo>,
    pub is_instrument: bool,  // true si category == "Instrument"
}

/// Directorios estándar de VST3 en Linux.
const VST3_PATHS: &[&str] = &[
    "~/.vst3",
    "/usr/lib/vst3",
    "/usr/local/lib/vst3",
];

/// Escanea recursivamente los directorios estándar y devuelve (ruta, nombre) de cada .vst3.
pub fn scan() -> Vec<Vst3Ref> {
    let mut out = Vec::new();
    
    // Función recursiva para buscar archivos .vst3 en un directorio y sus subdirectorios
    fn scan_recursive(dir: &PathBuf, out: &mut Vec<(String, String)>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                
                // Si es un directorio, recursar
                if p.is_dir() {
                    scan_recursive(&p, out);
                }
                // Si es un archivo .vst3, añadirlo
                else if p.extension().and_then(|e| e.to_str()) == Some("vst3") {
                    if let Some(name) = p.file_stem().and_then(|s| s.to_str()) {
                        out.push((p.to_string_lossy().into_owned(), name.to_string()));
                    }
                }
            }
        }
    }
    
    // Buscar en cada directorio estándar
    for path_str in VST3_PATHS {
        let path = if path_str.starts_with('~') {
            if let Some(home) = std::env::var("HOME").ok() {
                PathBuf::from(path_str.replacen('~', &home, 1))
            } else { continue; }
        } else {
            PathBuf::from(path_str)
        };
        
        if !path.exists() { continue; }
        scan_recursive(&path, &mut out);
    }
    
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

/// Carga y activa un plugin VST3.
pub fn load(path: &str, sample_rate: u32, max_frames: u32)
    -> Result<Vst3Slot, String>
{
    let mut plugin = simple::load_plugin_with_settings(path, sample_rate as f64, max_frames as usize)
        .map_err(|e| format!("load_plugin: {}", e))?;
    
    let info = plugin.info();
    let is_instrument = info.category.to_lowercase().contains("instrument");
    
    // Crear buffers de audio reutilizables.
    let buffers = plugin.create_bus_audio_buffers(max_frames as usize)
        .map_err(|e| format!("create_bus_audio_buffers: {}", e))?;
    
    let params = describe_params(&plugin);
    
    plugin.start_processing()
        .map_err(|e| format!("start_processing: {}", e))?;
    
    Ok(Vst3Slot { plugin, buffers, params, is_instrument })
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

/// Envía un evento MIDI al plugin en un sample offset específico (RT-safe).
pub fn send_midi(slot: &mut Vst3Slot, event: MidiEvent, sample_offset: i32) {
    let _ = slot.plugin.send_midi_event_at(event, sample_offset);
}

/// Actualiza un parámetro en tiempo real (RT-safe).
pub fn set_param(slot: &mut Vst3Slot, param_id: u32, value: f64, sample_offset: i32) {
    let _ = slot.plugin.set_parameter_at(param_id, value.clamp(0.0, 1.0), sample_offset);
}
