// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! GrooveKit — configuración y estado en vivo de las 25 celdas del drum kit.
//!
//! Equivalente al `Kit` de Caja. Separa claramente:
//! - `PadCfg`: configuración persistente (parámetros, banco de semillas,
//!   muestra bloqueada, gain/pan/mute/solo, polifonía, choke).
//! - `PadLive`: estado volátil por celda que cambia en vivo (semilla actual,
//!   slot actual, modo) y que el audio thread lee sin locks vía TripleBuffer.
//!
//! Regla de oro: el audio thread NUNCA toca el `Mutex<[PadCfg]>`. Solo lee
//! el `TripleBuffer<[PadLive]>` y los `Atomic*` de estado rápido.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use caja_core::kit::{ParamCfg, SEED_MAX, NUM_PADS, P_SAMPLE};
use lanzadera_core::TripleBuffer;

/// Número de celdas del kit. Alias para los crates que no quieren depender
/// directamente de `caja_core` (controller, ui).
pub const NUM_CELLS: usize = NUM_PADS;

/// Número de semillas por banco (el `SLOTS` de Caja).
pub const SLOTS: usize = 8;

/// Configuración persistente de un pad (guardada en el proyecto).
/// Los tipos y campos coinciden con `caja_core::kit::PadCfg` para facilitar
/// el port de más código de Caja.
#[derive(Clone, Debug)]
pub struct PadCfg {
    /// Ganancia en milésimas (1000 = 0 dB, máx. 2000 = +6 dB).
    pub gain: u16,
    /// Pan en milésimas (-1000..=1000).
    pub pan: i16,
    pub mute: bool,
    pub solo: bool,
    /// Polifonía 1..=4.
    pub poly: u8,
    /// Grupo de choke (0 = ninguno).
    pub choke_group: u8,
    /// Si el golpe de este pad corta las voces de su grupo.
    pub choker: bool,
    /// Banco de semillas (una por slot). El slot activo lo dicta `PadLive.slot`.
    pub bank: [u32; SLOTS],
    /// Semilla con la que se fijó el pin.
    pub pin_seed: u32,
    /// Muestra bloqueada por el usuario (ruta relativa a la raíz del kit).
    /// Si es `Some`, el slot activo usa esta muestra en lugar de la del banco.
    pub pin: Option<String>,
    /// Parámetros de dado/síntesis: [Muestra, Pitch, Decay, Filtro].
    pub param: [ParamCfg; 4],
    /// Re-tirar el dado cada N compases (0 = desactivado; 1 / 2 / 3 / 4).
    /// Por defecto 4: cambia la muestra cada 4 compases.
    pub rate: u8,
}

/// Valores por defecto de los cuatro parámetros (los mismos de Caja).
pub const DEFAULT_PARAM: [ParamCfg; 4] = [
    ParamCfg { base: 500, range: 1000, prob: 1000 }, // Muestra: todo el banco
    ParamCfg { base: 500, range: 60, prob: 1000 },   // Pitch: ±~1.4 semitonos
    ParamCfg { base: 1000, range: 0, prob: 1000 },   // Decay: sin recorte
    ParamCfg { base: 1000, range: 0, prob: 1000 },   // Filtro: abierto
];

impl Default for PadCfg {
    fn default() -> Self {
        PadCfg {
            gain: 1000,
            pan: 0,
            mute: false,
            solo: false,
            poly: 1,
            choke_group: 0,
            choker: false,
            bank: [0; SLOTS],
            pin_seed: 0,
            pin: None,
            param: DEFAULT_PARAM,
            rate: 4,
        }
    }
}

impl PadCfg {
    /// Gain en formato lineal (0.0 = silencio, 1.0 = unity, 2.0 = +6 dB).
    pub fn gain_linear(&self) -> f32 {
        (self.gain as f32) / 1000.0
    }
    /// Pan en formato -1.0..=1.0.
    pub fn pan_norm(&self) -> f32 {
        (self.pan as f32) / 1000.0
    }
}

/// Estado en vivo de un pad (lo que cambia al tocar/dar/sembrar).
/// POD: se copia por valor al `TripleBuffer`, sin allocs.
#[derive(Clone, Copy, Debug, Default)]
pub struct PadLive {
    /// Semilla actual del slot.
    pub seed: u32,
    /// Slot activo (0..SLOTS-1).
    pub slot: u8,
    /// Bits de modo: bit0 = locked_sample · bit1 = locked_pitch ·
    ///               bit2 = locked_decay · bit3 = locked_filter
    pub mode: u8,
}

/// Mezcla de un pad en formato POD list para el hilo de audio.
/// Sin String ni Mutex: se copia por valor desde el TripleBuffer.
#[derive(Clone, Copy, Debug)]
pub struct PadMix {
    /// Ganancia lineal (1.0 = unity, 2.0 = +6 dB).
    pub gain: f32,
    /// Pan -1.0..=1.0.
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
}

impl Default for PadMix {
    fn default() -> Self { PadMix { gain: 1.0, pan: 0.0, mute: false, solo: false } }
}

/// Parámetros POD que el hilo de audio necesita para reconstruir el
/// `caja_core::kit::PadCfg` de cada celda. Sin `String` ni `Mutex`: se copia
/// por valor desde el `TripleBuffer` dentro del callback.
#[derive(Clone, Copy, Debug)]
pub struct PadEngine {
    pub param: [ParamCfg; 4],
    pub bank: [u32; SLOTS],
    pub poly: u8,
    pub choke_group: u8,
    pub choker: bool,
    pub solo: bool,
    pub rate: u8,
}

impl Default for PadEngine {
    fn default() -> Self {
        PadEngine {
            param: DEFAULT_PARAM,
            bank: [0; SLOTS],
            poly: 2,
            choke_group: 0,
            choker: false,
            solo: false,
            rate: 4,
        }
    }
}

/// Estado del kit accesible desde cualquier hilo.
///
/// El audio thread solo lee `live` (TripleBuffer) y `any_solo` (atomic).
/// La UI/MK3 escribe `cfg` (Mutex) y publica los cambios en `live` vía
/// el helper `publish()`.
pub struct GrooveKit {
    /// Configuración persistente, mutada desde UI/MK3.
    cfg: Mutex<[PadCfg; NUM_PADS]>,
    /// Estado en vivo publicado para el audio thread (lock-free).
    live: TripleBuffer<[PadLive; NUM_PADS]>,
    /// Mezcla lock-free para el audio thread (gain/pan/mute/solo).
    mix: TripleBuffer<[PadMix; NUM_PADS]>,
    /// Parámetros de dado/síntesis lock-free para el audio thread.
    eng: TripleBuffer<[PadEngine; NUM_PADS]>,
    /// `true` si al menos un pad tiene `solo = true`.
    /// El audio thread lo lee sin locks para aplicar la lógica SOLO.
    pub any_solo: std::sync::atomic::AtomicBool,
    /// Contador de revisión: la UI lo compara para refrescar.
    rev: AtomicU32,
}

impl GrooveKit {
    pub fn new() -> Arc<Self> {
        // Valores iniciales tomados de Caja: banco de semillas, grupos de
        // choke (el hat cerrado corta al abierto) y rangos del dado.
        let cfg = std::array::from_fn(|i| {
            let d = caja_core::kit::PadCfg::default_for(i);
            PadCfg {
                bank: d.bank,
                choke_group: d.choke_group,
                choker: d.choker,
                param: d.cfg,
                ..PadCfg::default()
            }
        });
        // El TripleBuffer guarda el PadLive de ESTE módulo; el de caja-core
        // es otro tipo con los mismos campos, así que se convierte uno a uno.
        let live: [PadLive; NUM_PADS] = std::array::from_fn(|i| {
            let d = caja_core::kit::PadLive::default_for(i);
            PadLive { seed: d.seed, slot: d.slot, mode: d.mode }
        });
        let k = GrooveKit {
            cfg: Mutex::new(cfg),
            live: TripleBuffer::new(|| live),
            mix: TripleBuffer::new(|| [PadMix::default(); NUM_PADS]),
            eng: TripleBuffer::new(|| [PadEngine::default(); NUM_PADS]),
            any_solo: AtomicBool::new(false),
            rev: AtomicU32::new(0),
        };
        let k = Arc::new(k);
        // Groove Machine arranca con los cuatro parámetros bloqueados en
        // todas las celdas: el dado no varía nada hasta que se desbloquee.
        k.live.publish(|l| {
            for p in l.iter_mut() { p.mode = caja_core::kit::ALL_LOCK; }
        });
        k.publish();
        k
    }

    pub fn rev(&self) -> u32 { self.rev.load(Ordering::Relaxed) }

    /// Snapshot de mezcla para el HILO DE AUDIO: sin locks, sin allocs.
    /// Es una copia POD de 25 × 16 bytes.
    pub fn mix_snapshot(&self) -> [PadMix; NUM_PADS] { *self.mix.latest() }

    /// Snapshot de parámetros (dado/síntesis) para el HILO DE AUDIO: POD,
    /// sin locks y sin asignaciones.
    pub fn engine_snapshot(&self) -> [PadEngine; NUM_PADS] { *self.eng.latest() }

    // ── Lectura ───────────────────────────────────────────────────────

    /// Copia de la configuración de los 25 pads (snapshot para la UI).
    pub fn snapshot_cfg(&self) -> [PadCfg; NUM_PADS] {
        self.cfg.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Último estado en vivo publicado (snapshot para la UI).
    pub fn snapshot_live(&self) -> [PadLive; NUM_PADS] {
        *self.live.latest()
    }

    // ── Escritura: cada método muta `cfg` y vuelve a publicar ────────

    fn publish(&self) {
        let cfg = self.cfg.lock().unwrap_or_else(|e| e.into_inner());
        self.publish_inner(&cfg);
    }

    pub fn set_gain(&self, pad: usize, gain: u16) {
        let mut cfg = self.cfg.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = cfg.get_mut(pad) {
            c.gain = gain.min(2000);
            self.publish_inner(&cfg);
        }
    }

    pub fn set_pan(&self, pad: usize, pan: i16) {
        let mut cfg = self.cfg.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = cfg.get_mut(pad) {
            c.pan = pan.clamp(-1000, 1000);
            self.publish_inner(&cfg);
        }
    }

    pub fn set_mute(&self, pad: usize, mute: bool) {
        let mut cfg = self.cfg.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = cfg.get_mut(pad) {
            c.mute = mute;
            self.publish_inner(&cfg);
        }
    }

    pub fn toggle_mute(&self, pad: usize) {
        let mut cfg = self.cfg.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = cfg.get_mut(pad) {
            c.mute = !c.mute;
            self.publish_inner(&cfg);
        }
    }

    pub fn set_solo(&self, pad: usize, solo: bool) {
        let mut cfg = self.cfg.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = cfg.get_mut(pad) {
            c.solo = solo;
            self.publish_inner(&cfg);
        }
    }

    pub fn toggle_solo(&self, pad: usize) {
        let mut cfg = self.cfg.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = cfg.get_mut(pad) {
            c.solo = !c.solo;
            self.publish_inner(&cfg);
        }
    }

    /// Cicla el estado de una celda: normal → mute → solo → normal.
    pub fn cycle_pad_scene_state(&self, pad: usize) {
        let mut cfg = self.cfg.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(cell) = cfg.get_mut(pad) {
            if cell.mute {
                cell.mute = false;
                cell.solo = true;
            } else if cell.solo {
                cell.mute = false;
                cell.solo = false;
            } else {
                cell.mute = true;
                cell.solo = false;
            }
            self.publish_inner(&cfg);
        }
    }

    /// Semilla fija para un slot (cuando el usuario elige un sample concreto).
    pub fn set_slot_seed(&self, pad: usize, slot: usize, seed: u32) {
        let mut cfg = self.cfg.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = cfg.get_mut(pad) {
            if let Some(s) = c.bank.get_mut(slot) { *s = seed; }
            self.publish_inner(&cfg);
        }
    }

    /// Activa un slot concreto (lo que se oye al disparar el pad).
    pub fn set_active_slot(&self, pad: usize, slot: u8) {
        let mut live = *self.live.latest();  // copia mutable
        if let Some(p) = live.get_mut(pad) {
            p.slot = slot.min((SLOTS - 1) as u8);
        }
        self.live.publish(|l| *l = live);
        self.rev.fetch_add(1, Ordering::Relaxed);
    }

    /// Fija una muestra por ruta (pin). Guarda también la semilla activa:
    /// en Caja el pin solo manda mientras esa semilla siga siendo la actual.
    pub fn set_pin(&self, pad: usize, pin: Option<String>) {
        let live = *self.live.latest();
        let seed = live.get(pad).map(|l| l.seed).unwrap_or(0);
        let mut cfg = self.cfg.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = cfg.get_mut(pad) {
            c.pin = pin;
            c.pin_seed = seed;
            self.publish_inner(&cfg);
        }
    }

    /// Dado: elige un slot al azar y lo activa. Devuelve el slot elegido.
    pub fn roll_slot(&self, pad: usize) -> u8 {
        use std::time::{SystemTime, UNIX_EPOCH};
        let mut live = *self.live.latest();  // copia mutable
        if let Some(p) = live.get_mut(pad) {
            let t = SystemTime::now()
                .duration_since(UNIX_EPOCH).unwrap_or_default()
                .subsec_nanos();
            p.slot = (t as u8) % (SLOTS as u8);
        }
        self.live.publish(|l| *l = live);
        self.rev.fetch_add(1, Ordering::Relaxed);
        live.get(pad).map(|p| p.slot).unwrap_or(0)
    }

    /// Helper interno: publica tras mutar `cfg` (sin volver a tomar el lock).
    fn publish_inner(&self, cfg: &[PadCfg; NUM_PADS]) {
        let mix = std::array::from_fn(|i| PadMix {
            gain: cfg[i].gain_linear(),
            pan: cfg[i].pan_norm(),
            mute: cfg[i].mute,
            solo: cfg[i].solo,
        });

        let eng = std::array::from_fn(|i| PadEngine {
            param: cfg[i].param,
            bank: cfg[i].bank,
            poly: cfg[i].poly,
            choke_group: cfg[i].choke_group,
            choker: cfg[i].choker,
            solo: cfg[i].solo,
            rate: cfg[i].rate,
        });

        self.any_solo
            .store(cfg.iter().any(|pad| pad.solo), Ordering::Relaxed);
        self.mix.publish(|published| *published = mix);
        self.eng.publish(|published| *published = eng);

        // Al cambiar mezcla no cambia el estado live; se publica igualmente
        // para mantener el refresco del Inspector y el contador de revisión.
        let live = *self.live.latest();
        self.live.publish(|published| *published = live);
        self.rev.fetch_add(1, Ordering::Relaxed);
    }
}

// ════════════════════════════════════════════════════════════════════════
//  Modo CHORDS (Library): dado, locks por parámetro y rate de re-dado.
//  Estos métodos son de UI/controlador (pueden tomar el Mutex de `cfg`);
//  el hilo de audio solo consume `mix_snapshot()` y `engine_snapshot()`.
// ════════════════════════════════════════════════════════════════════════

/// Semilla nueva para el dado (distinta en cada llamada).
fn new_seed() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed) as u64;
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;
    ((t ^ n.wrapping_mul(0x9E37_79B9)) % (SEED_MAX as u64 + 1)) as u32
}

impl GrooveKit {
    /// Mutación lock-free del estado en vivo (seed / slot / mode).
    fn edit_live<R>(&self, f: impl FnOnce(&mut [PadLive; NUM_PADS]) -> R) -> R {
        let mut live = *self.live.latest();
        let r = f(&mut live);
        self.live.publish(|l| *l = live);
        self.rev.fetch_add(1, Ordering::Relaxed);
        r
    }

    /// Dado: semilla nueva para una celda.
    pub fn roll_seed(&self, pad: usize) -> u32 {
        let seed = new_seed();
        self.edit_live(|live| {
            if let Some(l) = live.get_mut(pad) { l.seed = seed; }
        });
        seed
    }

    /// Dado en todas las celdas a la vez.
    pub fn roll_all_seeds(&self) {
        self.edit_live(|live| {
            for (i, l) in live.iter_mut().enumerate() {
                l.seed = (new_seed() ^ (i as u32).wrapping_mul(7919)) % (SEED_MAX + 1);
            }
        });
    }

    /// Alterna el LOCK de un parámetro (P_SAMPLE / P_PITCH / P_DECAY /
    /// P_FILTER). Devuelve el estado resultante: true = bloqueado.
    pub fn toggle_lock(&self, pad: usize, param: usize) -> bool {
        let bit = 1u8 << param.min(3);
        self.edit_live(|live| {
            if let Some(l) = live.get_mut(pad) {
                l.mode ^= bit;
                l.mode & bit != 0
            } else {
                false
            }
        })
    }

    pub fn set_lock(&self, pad: usize, param: usize, on: bool) {
        let bit = 1u8 << param.min(3);
        self.edit_live(|live| {
            if let Some(l) = live.get_mut(pad) {
                l.mode = if on { l.mode | bit } else { l.mode & !bit };
            }
        });
    }

    /// ALL ROLL / ALL LOCK en las 25 celdas.
    pub fn set_all_modes(&self, mode: u8) {
        let m = mode & 0b1111;
        self.edit_live(|live| {
            for l in live.iter_mut() { l.mode = m; }
        });
    }

    /// Suma `delta` milésimas al valor base de un parámetro (0..=1000).
    /// No comprueba el lock: el lock solo detiene el dado, la edición manual
    /// es siempre válida (igual que los sliders del Inspector).
    pub fn bump_param_base(&self, pad: usize, param: usize, delta: i32) -> Option<u16> {
        let mut cfg = self.cfg.lock().unwrap_or_else(|e| e.into_inner());
        let c = cfg.get_mut(pad)?;
        let p = c.param.get_mut(param.min(3))?;
        p.base = (i32::from(p.base) + delta).clamp(0, 1000) as u16;
        let v = p.base;
        self.publish_inner(&cfg);
        Some(v)
    }

    /// Edita base / rango / probabilidad de un parámetro de la celda.
    pub fn set_param(&self, pad: usize, param: usize, value: ParamCfg) {
        let mut cfg = self.cfg.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = cfg.get_mut(pad) {
            if let Some(slot) = c.param.get_mut(param.min(3)) { *slot = value; }
            self.publish_inner(&cfg);
        }
    }

    /// Re-dado automático cada N compases (0 = desactivado).
    pub fn set_rate(&self, pad: usize, bars: u8) {
        let mut cfg = self.cfg.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = cfg.get_mut(pad) {
            c.rate = bars;
            self.publish_inner(&cfg);
        }
    }

    pub fn set_all_rates(&self, bars: u8) {
        let mut cfg = self.cfg.lock().unwrap_or_else(|e| e.into_inner());
        for c in cfg.iter_mut() { c.rate = bars; }
        self.publish_inner(&cfg);
    }

    /// Índice de muestra que daría la semilla actual (para la UI/Inspector).
    pub fn seed_sample_index(&self, pad: usize, bank_len: usize) -> Option<usize> {
        if bank_len == 0 { return None; }
        let live = *self.live.latest();
        let seed = live.get(pad).map(|l| l.seed).unwrap_or(0);
        let cfg = self.cfg.lock().unwrap_or_else(|e| e.into_inner());
        let Some(c) = cfg.get(pad) else { return None };
        let mut caja_cfg = caja_core::kit::PadCfg::default_for(pad);
        caja_cfg.cfg[P_SAMPLE] = c.param[P_SAMPLE];
        caja_core::kit::locked_index(&caja_cfg, seed, bank_len)
    }
}

#[cfg(test)]
mod chords_tests {
    use super::*;
    use caja_core::kit::{is_lock, ALL_LOCK, ALL_ROLL, P_DECAY, P_FILTER, P_PITCH};

    #[test]
    fn el_dado_cambia_la_semilla_de_la_celda() {
        let kit = GrooveKit::new();
        let antes = kit.snapshot_live()[3].seed;
        let despues = kit.roll_seed(3);
        assert_ne!(antes, despues, "el dado debe estrenar semilla");
        assert_eq!(kit.snapshot_live()[3].seed, despues);
    }

    #[test]
    fn los_locks_alternan_por_parametro() {
        let kit = GrooveKit::new();
        kit.set_all_modes(ALL_ROLL);
        assert!(kit.toggle_lock(2, P_PITCH));
        let mode = kit.snapshot_live()[2].mode;
        assert!(is_lock(mode, P_PITCH));
        assert!(!is_lock(mode, P_DECAY));
        assert!(!is_lock(mode, P_FILTER));
        assert!(!kit.toggle_lock(2, P_PITCH));
        assert!(!is_lock(kit.snapshot_live()[2].mode, P_PITCH));
    }

    #[test]
    fn all_lock_y_all_roll_cambian_todas_las_celdas() {
        let kit = GrooveKit::new();
        kit.set_all_modes(ALL_LOCK);
        assert!(kit.snapshot_live().iter().all(|l| l.mode == ALL_LOCK));
        kit.set_all_modes(ALL_ROLL);
        assert!(kit.snapshot_live().iter().all(|l| l.mode == ALL_ROLL));
    }

    #[test]
    fn los_parametros_viajan_al_snapshot_del_audio() {
        let kit = GrooveKit::new();
        kit.set_param(1, P_PITCH, ParamCfg { base: 250, range: 400, prob: 500 });
        kit.set_rate(1, 4);
        let eng = kit.engine_snapshot();
        assert_eq!(eng[1].param[P_PITCH].base, 250);
        assert_eq!(eng[1].param[P_PITCH].range, 400);
        assert_eq!(eng[1].param[P_PITCH].prob, 500);
        assert_eq!(eng[1].rate, 4);
        assert_eq!(eng[0].rate, 0);
        kit.set_all_rates(2);
        assert!(kit.engine_snapshot().iter().all(|e| e.rate == 2));
    }

    #[test]
    fn el_pin_guarda_la_semilla_activa() {
        let kit = GrooveKit::new();
        let seed = kit.roll_seed(4);
        kit.set_pin(4, Some("kicks/Kick_01.wav".to_string()));
        let cfg = kit.snapshot_cfg();
        assert_eq!(cfg[4].pin.as_deref(), Some("kicks/Kick_01.wav"));
        assert_eq!(cfg[4].pin_seed, seed);
    }
}

#[cfg(test)]
mod mixer_default_tests {
    use super::*;

    #[test]
    fn el_kit_arranca_con_todo_bloqueado() {
        let kit = GrooveKit::new();
        assert!(kit.snapshot_live().iter().all(|l| l.mode == caja_core::kit::ALL_LOCK));
    }

    #[test]
    fn gain_y_pan_de_la_gui_llegan_al_snapshot_de_audio() {
        let kit = GrooveKit::new();
        kit.set_gain(3, 250);
        kit.set_pan(3, -800);
        let mix = kit.mix_snapshot();
        assert_eq!(mix[3].gain, 0.25);
        assert_eq!(mix[3].pan, -0.8);
    }
}
