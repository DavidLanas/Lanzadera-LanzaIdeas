//! Estado compartido de la biblioteca de muestras de Groove Machine.
//!
//! Equivale a la pestaña Library de Caja: carpeta raíz, tope de muestras por
//! pad (control de RAM), duración máxima (excluye loops) y bandeja de
//! "Sin clasificar" con asignaciones manuales.
//!
//! Singleton por aplicación: hay un único `CajaStore` y un único hilo loader.
//! Solo lo tocan el hilo loader y el hilo de UI — nunca el de audio.
//!
//! La configuración se persiste en `~/.config/lanzadera/groove-library.cfg`
//! en texto plano (legible y editable a mano, sin dependencias).

use std::sync::{Arc, Mutex, OnceLock};

use caja_core::kit::NUM_PADS;
use caja_core::library::{Library, DEFAULT_BANK, MAX_BANK, MIN_BANK};

/// Duración máxima por defecto: 4 s deja pasar crashes/rides largos
/// pero excluye la mayoría de loops de batería completa.
pub const DEFAULT_MAX_LEN_MS: u32 = 4000;

pub struct GrooveLibrary {
    /// Biblioteca clasificada (celdas por pad + bandeja de sin clasificar).
    pub lib: Library,
    /// Asignaciones manuales `(ruta_relativa, pad)`: mandan sobre el clasificador.
    pub assign: Vec<(String, usize)>,
    /// Muestras por pad (8..=128). Control de RAM.
    pub bank_cap: usize,
    /// Duración máxima en ms. 0 = sin límite. Excluye loops.
    pub max_len_ms: u32,
    /// Carpeta raíz activa (la que el loader está usando).
    pub root: String,
    /// Carpeta raíz pedida por el usuario (vacío = usar env/default).
    pub root_request: String,
    /// Todas las rutas relativas encontradas (tras filtrar por duración).
    pub files: Vec<String>,
    /// Muestras realmente cargadas por pad.
    pub loaded: [usize; NUM_PADS],
    /// Archivos descartados por exceder `max_len_ms`.
    pub skipped_long: usize,
    /// true si se usó la estructura de carpetas numeradas 00/..24/
    pub numbered: bool,
    /// La UI la sube para pedir re-escaneo al loader.
    pub dirty: bool,
    /// Muestra fijada por celda: `(índice dentro del banco, ruta relativa)`.
    /// El loader la resuelve a `CajaStore::pin_idx` tras cada reconstrucción.
    pub pins: [Option<(usize, String)>; NUM_PADS],
    /// La UI lo sube al cambiar un pin; el loader lo baja al resolverlo.
    pub pins_dirty: bool,
    /// Muestras marcadas como favoritas (rutas relativas).
    pub favs: Vec<String>,
}

impl Default for GrooveLibrary {
    fn default() -> Self {
        GrooveLibrary {
            lib: Library::empty(),
            assign: Vec::new(),
            bank_cap: DEFAULT_BANK,
            max_len_ms: DEFAULT_MAX_LEN_MS,
            root: String::new(),
            root_request: String::new(),
            files: Vec::new(),
            loaded: [0; NUM_PADS],
            skipped_long: 0,
            numbered: false,
            dirty: false,
            pins: std::array::from_fn(|_| None),
            // Al arrancar hay que resolver los pins leídos del archivo.
            pins_dirty: true,
            favs: Vec::new(),
        }
    }
}

fn cfg_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::PathBuf::from(home).join(".config/lanzadera/groove-library.cfg")
}

impl GrooveLibrary {
    // ── Persistencia (texto plano, sin dependencias) ──────────────────

    pub fn load_or_default() -> Self {
        let mut g = GrooveLibrary::default();
        let Ok(txt) = std::fs::read_to_string(cfg_path()) else { return g };
        for line in txt.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            let Some((k, v)) = line.split_once('=') else { continue };
            match k.trim() {
                "root"       => g.root_request = v.trim().to_string(),
                "bank_cap"   => if let Ok(n) = v.trim().parse::<usize>() { g.bank_cap = n.clamp(MIN_BANK, MAX_BANK); },
                "max_len_ms" => if let Ok(n) = v.trim().parse::<u32>()   { g.max_len_ms = n; },
                "fav" => {
                    let rel = v.trim();
                    if !rel.is_empty() { g.favs.push(rel.to_string()); }
                }
                "pin" => {
                    // pin=<pad>|<índice>|<ruta relativa>
                    let mut it = v.split('|');
                    match (it.next(), it.next(), it.next()) {
                        (Some(p), Some(i), Some(rel)) => {
                            if let (Ok(pad), Ok(idx)) =
                                (p.trim().parse::<usize>(), i.trim().parse::<usize>())
                            {
                                if pad < NUM_PADS {
                                    g.pins[pad] = Some((idx, rel.trim().to_string()));
                                }
                            }
                        }
                        _ => {}
                    }
                }
                "assign" => {
                    // assign=<pad>|<ruta relativa>
                    if let Some((pad, rel)) = v.split_once('|') {
                        if let Ok(p) = pad.trim().parse::<usize>() {
                            if p < NUM_PADS { g.assign.push((rel.trim().to_string(), p)); }
                        }
                    }
                }
                _ => {}
            }
        }
        g
    }

    pub fn save(&self) {
        let p = cfg_path();
        if let Some(dir) = p.parent() { let _ = std::fs::create_dir_all(dir); }
        let mut s = String::from("# Configuración de la biblioteca de Groove Machine\n");
        s.push_str(&format!("root={}\n", self.root_request));
        s.push_str(&format!("bank_cap={}\n", self.bank_cap));
        s.push_str(&format!("max_len_ms={}\n", self.max_len_ms));
        for (rel, pad) in &self.assign {
            s.push_str(&format!("assign={}|{}\n", pad, rel));
        }
        for (pad, pin) in self.pins.iter().enumerate() {
            if let Some((idx, rel)) = pin {
                s.push_str(&format!("pin={}|{}|{}\n", pad, idx, rel));
            }
        }
        for rel in &self.favs {
            s.push_str(&format!("fav={}\n", rel));
        }
        let _ = std::fs::write(p, s);
    }

    // ── Controles (todos marcan dirty y persisten) ────────────────────

    /// Cambia la carpeta raíz de escaneo.
    pub fn set_root(&mut self, path: &str) {
        let path = path.trim().to_string();
        if path != self.root_request {
            self.root_request = path;
            self.dirty = true;
            self.save();
        }
    }

    /// Tope de muestras por pad (8..=128) — control de RAM.
    pub fn set_bank_cap(&mut self, cap: usize) {
        let cap = cap.clamp(MIN_BANK, MAX_BANK);
        if cap != self.bank_cap {
            self.bank_cap = cap;
            self.dirty = true;
            self.save();
        }
    }

    /// Duración máxima en ms (0 = sin límite). Excluye loops.
    pub fn set_max_len_ms(&mut self, ms: u32) {
        if ms != self.max_len_ms {
            self.max_len_ms = ms;
            self.dirty = true;
            self.save();
        }
    }

    /// Asigna una muestra huérfana a un pad.
    pub fn assign_to_pad(&mut self, rel: &str, pad: usize) {
        let rel = caja_core::library::norm_rel(rel);
        let pad = pad.min(NUM_PADS - 1);
        if let Some(e) = self.assign.iter_mut().find(|(r, _)| *r == rel) {
            e.1 = pad;
        } else {
            self.assign.push((rel, pad));
        }
        self.dirty = true;
        self.save();
    }

    /// Quita una asignación manual (vuelve al clasificador).
    pub fn unassign(&mut self, rel: &str) {
        let rel = caja_core::library::norm_rel(rel);
        self.assign.retain(|(r, _)| *r != rel);
        self.dirty = true;
        self.save();
    }

    pub fn request_rescan(&mut self) { self.dirty = true; }

    /// Muestra fijada de una celda. El índice viaja junto a la ruta para
    /// poder resolverla también en el modo de carpetas numeradas.
    pub fn set_pin(&mut self, pad: usize, pin: Option<(usize, String)>) {
        if pad >= NUM_PADS { return; }
        if self.pins[pad] != pin {
            self.pins[pad] = pin;
            self.pins_dirty = true;
        }
    }

    /// Alterna una muestra en favoritos. Devuelve el estado resultante.
    pub fn toggle_fav(&mut self, rel: &str) -> bool {
        let rel = caja_core::library::norm_rel(rel);
        if let Some(pos) = self.favs.iter().position(|r| *r == rel) {
            self.favs.remove(pos);
            self.save();
            return false;
        }
        self.favs.push(rel);
        self.save();
        true
    }

    pub fn is_fav(&self, rel: &str) -> bool {
        let rel = caja_core::library::norm_rel(rel);
        self.favs.iter().any(|r| *r == rel)
    }

    pub fn unclassified(&self) -> &[String] { &self.lib.unclassified }
    pub fn total_classified(&self) -> usize { self.lib.total_classified() }
    pub fn total_loaded(&self) -> usize { self.loaded.iter().sum() }
}

static SHARED: OnceLock<Arc<Mutex<GrooveLibrary>>> = OnceLock::new();

/// Acceso al estado compartido (carga la config persistida en el primer uso).
pub fn shared() -> Arc<Mutex<GrooveLibrary>> {
    SHARED
        .get_or_init(|| Arc::new(Mutex::new(GrooveLibrary::load_or_default())))
        .clone()
}
