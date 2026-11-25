// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! GrooveModel — equivalente al `Model` de Caja, pero sin nih-plug.
//!
//! Contiene la `Session` completa de caja-core (patrón, géneros, rack de 32
//! patrones, historial, mapas de notas) y publica el `Schedule` resultante
//! en el `CajaSchedulerShared` que ya consume el hilo de audio.
//!
//! Regla de oro: este módulo vive en el hilo de UI / controlador.
//! El hilo de audio NUNCA lo toca: solo lee el triple buffer del scheduler.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use caja_core::pattern::Pattern;
use caja_core::pieces::NUM_PIECES;
use caja_core::schedule::Schedule;
use caja_core::session::{ParamSnap, Session};

use crate::caja_drums::CajaSchedulerShared;

/// Copia ligera de lo que la GUI necesita en un fotograma.
/// Se refresca solo cuando `rev()` cambia, no en cada frame.
pub struct GrooveSnapshot {
    pub pattern: Pattern,
    pub notes: [u8; NUM_PIECES],
    pub p: ParamSnap,
    pub reading: String,
    pub genre_name: String,
    /// Nombres de todos los géneros (para el combo del inspector).
    pub genre_names: Vec<String>,
    /// Familia de cada género, en el mismo orden que `genre_names`.
    /// Sirve para agrupar el selector sin alterar los índices.
    pub genre_families: Vec<String>,
    /// Slot activo del rack (0..31).
    pub active: usize,
    /// Slots con contenido.
    pub filled: [bool; 32],
    /// Nombres cortos de cada slot.
    pub names: [String; 32],
    pub message: String,
}

pub struct GrooveModel {
    session: Mutex<Session>,
    /// Contador de revisiones: la GUI lo compara para saber si refrescar.
    rev: AtomicU64,
    /// Destino de cada patrón recalculado.
    sched: Arc<CajaSchedulerShared>,
}

impl GrooveModel {
    pub fn new(sched: Arc<CajaSchedulerShared>) -> Self {
        // Géneros y mapas: respetar el override del usuario
        // (CAJA_GENRES / <config>/genres.json, CAJA_DRUM_MAPS / maps/).
        // Session::builtin() ignoraba esos archivos por completo.
        let genres = caja_core::paths::load_genres();
        let maps = caja_core::paths::load_maps();
        for e in &genres.errors {
            eprintln!("[groove] géneros: {}", e);
        }
        for e in &maps.errors {
            eprintln!("[groove] mapas: {}", e);
        }
        let n_genres = genres.value.len();
        let g_origin = genres.origin.clone();
        let mut session = Session::new(genres, maps);
        println!("[groove] {} géneros · {}", n_genres, g_origin);
        // Patrón inicial: así Groove Machine suena nada más abrir la ventana.
        let cur = session.snap();
        let _ = session.reroll(cur, false);

        let m = GrooveModel {
            session: Mutex::new(session),
            rev: AtomicU64::new(1),
            sched,
        };
        m.publish_locked();
        m
    }

    pub fn rev(&self) -> u64 {
        self.rev.load(Ordering::Relaxed)
    }

    // ── Interno ────────────────────────────────────────────────────────

    /// Recalcula el Schedule desde la sesión y lo publica. Sube la revisión.
    fn publish(&self, s: &Session) {
        let (loop_ticks, hits) = s.hits();
        let mut sch = Schedule::default();
        sch.fill(loop_ticks, &hits);
        self.sched.set_schedule(sch);
        self.rev.fetch_add(1, Ordering::Relaxed);
    }

    fn publish_locked(&self) {
        let s = self.session.lock().unwrap_or_else(|e| e.into_inner());
        self.publish(&s);
    }

    /// Aplica una mutación de `ParamSnap` y sincroniza la sesión.
    fn edit_snap(&self, f: impl FnOnce(&mut ParamSnap)) {
        let mut s = self.session.lock().unwrap_or_else(|e| e.into_inner());
        let mut p = s.snap();
        f(&mut p);
        s.sync(p);
        self.publish(&s);
    }

    // ── Lectura ────────────────────────────────────────────────────────

    pub fn snapshot(&self) -> GrooveSnapshot {
        let s = self.session.lock().unwrap_or_else(|e| e.into_inner());
        let p = s.snap();
        let gi = s.genre_index(&p);
        GrooveSnapshot {
            pattern: s.pattern.clone(),
            notes: s.notes,
            p,
            reading: s.reading(),
            genre_name: s.genre_name(&p).to_string(),
            genre_names: s.genres.genres.iter().map(|g| g.name.clone()).collect(),
            genre_families: s.genres.genres.iter().map(|g| g.family.clone()).collect(),
            active: s.rack.active,
            filled: std::array::from_fn(|i| !s.rack.entries[i].is_empty()),
            names: std::array::from_fn(|i| s.rack.entries[i].name.clone()),
            message: s.message.clone(),
        }
        .with_genre_index(gi)
    }

    // ── Generación (RESTART / ERASE / FOLLOW) ──────────────────────────

    /// RESTART: regenerar con la semilla actual (history=true guarda tarjeta).
    pub fn reroll(&self, history: bool) {
        let mut s = self.session.lock().unwrap_or_else(|e| e.into_inner());
        let cur = s.snap();
        let _ = s.reroll(cur, history);
        self.publish(&s);
    }

    /// ERASE: generar desde una frase en lenguaje natural.
    pub fn generate(&self, phrase: &str) {
        let mut s = self.session.lock().unwrap_or_else(|e| e.into_inner());
        let cur = s.snap();
        let _ = s.generate(phrase, cur);
        self.publish(&s);
    }

    /// FOLLOW: seleccionar género por índice absoluto.
    pub fn set_genre(&self, idx: usize) {
        let mut s = self.session.lock().unwrap_or_else(|e| e.into_inner());
        let cur = s.snap();
        let _ = s.set_genre(idx, cur);
        self.publish(&s);
    }

    /// FOLLOW: género siguiente / anterior (con wrap).
    pub fn cycle_genre(&self, delta: i8) {
        let mut s = self.session.lock().unwrap_or_else(|e| e.into_inner());
        let n = s.genres.genres.len().max(1) as i32;
        let cur = s.snap();
        let next = (cur.genre + delta as i32).rem_euclid(n) as usize;
        let _ = s.set_genre(next, cur);
        self.publish(&s);
    }

    /// Dado: semilla nueva al azar.
    pub fn randomize_seed(&self) {
        let mut s = self.session.lock().unwrap_or_else(|e| e.into_inner());
        let cur = s.snap();
        let n = caja_core::session::random_seed();
        let mut p = cur;
        p.seed = n;
        s.sync(p);
        let _ = s.reroll(p, false);
        self.publish(&s);
    }

    // ── Parámetros del patrón (inspector / encoder / SWING) ────────────

    pub fn set_swing(&self, v: i32)   { self.edit_snap(|p| p.swing   = v.clamp(0, 100)); }
    pub fn set_accent(&self, v: i32)  { self.edit_snap(|p| p.accent  = v.clamp(0, 100)); }
    pub fn set_human(&self, v: i32)   { self.edit_snap(|p| p.human   = v.clamp(0, 100)); }
    pub fn set_density(&self, v: i32) { self.edit_snap(|p| p.density = v.clamp(0, 100)); }
    pub fn set_seed(&self, v: i32)    { self.edit_snap(|p| p.seed    = v.max(0)); }

    /// VARIATION: ciclar la extensión del patrón (índice en BARS_CHOICES).
    pub fn set_bars_idx(&self, idx: i32) {
        let n = caja_core::session::BARS_CHOICES.len() as i32;
        self.edit_snap(|p| p.bars_idx = idx.clamp(0, n - 1));
    }

    pub fn cycle_bars(&self, delta: i8) {
        let n = caja_core::session::BARS_CHOICES.len() as i32;
        self.edit_snap(|p| p.bars_idx = (p.bars_idx + delta as i32).rem_euclid(n));
    }

    /// Delta genérico para el encoder del MK3 (modo SWING).
    /// 1=swing 2=accent 3=human 4=density
    pub fn bump(&self, which: u8, delta: i32) {
        self.edit_snap(|p| match which {
            1 => p.swing   = (p.swing   + delta).clamp(0, 100),
            2 => p.accent  = (p.accent  + delta).clamp(0, 100),
            3 => p.human   = (p.human   + delta).clamp(0, 100),
            4 => p.density = (p.density + delta).clamp(0, 100),
            _ => {}
        });
    }

    // ── Edición de celdas (step sequencer) ─────────────────────────────

    pub fn toggle_cell(&self, piece: usize, step: usize) {
        let mut s = self.session.lock().unwrap_or_else(|e| e.into_inner());
        if step < s.pattern.steps() {
            s.toggle_cell(piece, step, false);
            self.publish(&s);
        }
    }

    pub fn toggle_ghost(&self, piece: usize, step: usize) {
        let mut s = self.session.lock().unwrap_or_else(|e| e.into_inner());
        if step < s.pattern.steps() {
            s.toggle_ghost(piece, step);
            self.publish(&s);
        }
    }

    pub fn set_cell_vel(&self, piece: usize, step: usize, vel: i32) {
        let mut s = self.session.lock().unwrap_or_else(|e| e.into_inner());
        if step < s.pattern.steps() {
            s.set_cell_vel(piece, step, vel);
            self.publish(&s);
        }
    }

    pub fn set_note(&self, piece: usize, note: u8) {
        let mut s = self.session.lock().unwrap_or_else(|e| e.into_inner());
        s.set_note(piece, note.min(127));
        self.publish(&s);
    }

    // ── Rack de patrones (botón PATTERN) ───────────────────────────────

    pub fn select_pattern(&self, idx: usize) {
        let mut s = self.session.lock().unwrap_or_else(|e| e.into_inner());
        let _ = s.select_pattern(idx.min(31));
        self.publish(&s);
    }

    pub fn set_pattern_name(&self, name: &str) {
        let mut s = self.session.lock().unwrap_or_else(|e| e.into_inner());
        s.set_active_name(name);
        self.rev.fetch_add(1, Ordering::Relaxed);
    }

    pub fn clear_pattern(&self, idx: usize) {
        let mut s = self.session.lock().unwrap_or_else(|e| e.into_inner());
        s.clear_pattern_slot(idx.min(31));
        self.publish(&s);
    }

    // ── Export / persistencia ──────────────────────────────────────────

    /// STAR: exportar .mid
    pub fn export_midi(&self, separate: bool, use_gm: bool) -> Result<std::path::PathBuf, String> {
        let mut s = self.session.lock().unwrap_or_else(|e| e.into_inner());
        s.export(separate, use_gm)
    }

    pub fn to_json(&self) -> String {
        let s = self.session.lock().unwrap_or_else(|e| e.into_inner());
        s.to_state_json()
    }

    pub fn load_json(&self, src: &str) -> Result<(), String> {
        let mut s = self.session.lock().unwrap_or_else(|e| e.into_inner());
        let cur = s.snap();
        s.load_state_json(src, cur)?;
        self.publish(&s);
        Ok(())
    }
}

impl GrooveSnapshot {
    /// Guarda el índice de género resuelto (helper interno).
    fn with_genre_index(mut self, _gi: usize) -> Self {
        self.p.genre = self.p.genre.max(0);
        self
    }
}
