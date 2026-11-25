//! Preferencias de la app: ~/.config/lanzadera/config.json (o $XDG_CONFIG_HOME).
//! Una sola copia en memoria; `update` modifica y guarda en disco de forma atómica.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

fn yes() -> bool { true }
fn one() -> f32 { 1.0 }

/// Un controlador MIDI que se reconecta solo al aparecer (enchufarlo o abrir la app).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MidiAuto {
    /// "keylab" | "mk3" | "generic"
    pub profile: String,
    /// Nombre del puerto de entrada tal como lo lista el sistema.
    pub port: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// Buscar también en las carpetas estándar del sistema.
    #[serde(default = "yes")]
    pub use_default_plugin_paths: bool,
    /// Carpetas CLAP añadidas por el usuario.
    #[serde(default)]
    pub clap_paths: Vec<String>,
    /// Carpetas VST3 añadidas por el usuario.
    #[serde(default)]
    pub vst3_paths: Vec<String>,
    #[serde(default)]
    pub midi_auto: Vec<MidiAuto>,
    /// Zoom de la interfaz (1.0 = 100 %).
    #[serde(default = "one")]
    pub ui_scale: f32,
    /// Latencia de ida y vuelta (ms) que se compensa en el audio grabado. None = automática
    /// (dos buffers de audio). También se puede forzar con LANZADERA_REC_LATENCY_MS.
    #[serde(default)]
    pub rec_latency_ms: Option<f32>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            use_default_plugin_paths: true,
            clap_paths: Vec::new(),
            vst3_paths: Vec::new(),
            midi_auto: Vec::new(),
            ui_scale: 1.0,
            rec_latency_ms: None,
        }
    }
}

pub fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("lanzadera"))
}

fn config_file() -> Option<PathBuf> { Some(config_dir()?.join("config.json")) }

static CFG: OnceLock<RwLock<Config>> = OnceLock::new();

fn cell() -> &'static RwLock<Config> {
    CFG.get_or_init(|| RwLock::new(load_from_disk()))
}

fn load_from_disk() -> Config {
    config_file()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_to_disk(c: &Config) {
    let Some(p) = config_file() else { return };
    if let Some(d) = p.parent() { let _ = std::fs::create_dir_all(d); }
    let tmp = p.with_extension("json.tmp");
    if let Ok(s) = serde_json::to_string_pretty(c) {
        if std::fs::write(&tmp, s).is_ok() { let _ = std::fs::rename(&tmp, &p); }
    }
}

/// Copia de las preferencias actuales.
pub fn get() -> Config {
    cell().read().map(|c| c.clone()).unwrap_or_default()
}

/// Modifica las preferencias y las guarda en disco.
pub fn update<F: FnOnce(&mut Config)>(f: F) {
    let snapshot = {
        let mut w = match cell().write() { Ok(w) => w, Err(e) => e.into_inner() };
        f(&mut w);
        w.clone()
    };
    save_to_disk(&snapshot);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_vacio_da_valores_por_defecto() {
        let c: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(c, Config::default());
        assert!(c.use_default_plugin_paths);
        assert_eq!(c.ui_scale, 1.0);
    }

    #[test]
    fn ida_y_vuelta() {
        let mut c = Config::default();
        c.clap_paths.push("/opt/clap".into());
        c.midi_auto.push(MidiAuto { profile: "generic".into(), port: "Nektar".into() });
        c.ui_scale = 1.25;
        let back: Config = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn campos_desconocidos_no_rompen() {
        let c: Config = serde_json::from_str(r#"{"clap_paths":["/a"],"futuro":123}"#).unwrap();
        assert_eq!(c.clap_paths, vec!["/a".to_string()]);
    }
}
