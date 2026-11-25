// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! Wrapper del Scheduler de caja-core para Lanzadera.
//! Convierte el calendario de golpes en triggers de pads sincronizados al
//! transporte del launcher.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::Mutex;

use caja_core::scheduler::{RunSettings, Scheduler, Transport};
use caja_core::schedule::Schedule;

/// Estado compartido del scheduler de Caja (vive en SessionShared).
pub struct CajaSchedulerShared {
    /// Calendario de golpes. La UI escribe; el audio lee (clona el Arc).
    pub schedule: Mutex<Arc<Schedule>>,
    /// Configuración de ejecución.
    pub settings: Mutex<RunSettings>,
    /// Activo o inactivo (toggle global).
    pub enabled: AtomicBool,
    /// Step actual (0..15) publicado por el audio, leído por la UI/LEDs.
    /// 255 = parado.
    pub current_step: AtomicU8,
    /// Paso absoluto dentro del bucle completo; u32::MAX = parado.
    pub current_abs_step: AtomicU32,
    /// Tempo fijo para el reloj libre (run_mode=1). Se copia desde el launcher.
    pub free_bpm: AtomicU8,
    /// Ciclo de Groove en ticks, publicado para el motor de clips.
    /// Se lee sin lock en el callback RT.
    pub groove_loop_ticks: AtomicU32,
}

impl Default for CajaSchedulerShared {
    fn default() -> Self {
        // Por defecto: un patrón vacío de 1 compás, 16 pasos, tempo 120.
        let mut sch = Schedule::default();
        sch.loop_ticks = 1920; // 1 compás = 1920 ticks
        // hits y step_vel ya están inicializados por Schedule::default()
        CajaSchedulerShared {
            schedule: Mutex::new(Arc::new(sch)),
            settings: Mutex::new(RunSettings { free_bpm: 120, run_mode: 1, ..Default::default() }),
            enabled: AtomicBool::new(false),
            current_step: AtomicU8::new(255),
            current_abs_step: AtomicU32::new(u32::MAX),
            free_bpm: AtomicU8::new(120),
            groove_loop_ticks: AtomicU32::new(1920),
        }
    }
}

impl CajaSchedulerShared {
    /// Publica un nuevo calendario (llamado desde la UI).
    pub fn set_schedule(&self, sch: Schedule) {
        self.groove_loop_ticks.store(sch.loop_ticks.max(1), Ordering::Relaxed);
        *self.schedule.lock().unwrap() = Arc::new(sch);
    }

    /// Publica el tempo del launcher (run_mode=1).
    pub fn set_bpm(&self, bpm: u8) {
        self.free_bpm.store(bpm, Ordering::Relaxed);
        if let Ok(mut s) = self.settings.lock() { s.free_bpm = bpm as i32; }
    }

    /// Toggle global.
    pub fn toggle(&self) -> bool {
        let new = !self.enabled.load(Ordering::Relaxed);
        self.enabled.store(new, Ordering::Relaxed);
        new
    }
}

/// Wrapper del Scheduler. Vive en el Renderer (hilo de audio).
pub struct CajaScheduler {
    inner: Scheduler,
    shared: Arc<CajaSchedulerShared>,
}

impl CajaScheduler {
    pub fn new(shared: Arc<CajaSchedulerShared>, sample_rate: f64) -> Self {
        let mut inner = Scheduler::new();
        inner.activate(sample_rate);
        CajaScheduler { inner, shared }
    }

    /// Procesa un bloque y devuelve los triggers de pads como array fijo.
    /// Cada elemento es (frame_offset, pad_index, velocity).
    /// Los elementos con vel=0.0 deben ignorarse.
    pub fn process(
        &mut self,
        frames: usize,
        transport_playing: bool,
        tempo: f64,
        song_beats: f64,
    ) -> [(u32, u8, f32); 64] {
        let mut out_buf = [(0u32, 0u8, 0.0f32); 64];

        // Sincronizar el tempo del launcher al scheduler.
        if let Ok(mut s) = self.shared.settings.lock() {
            s.free_bpm = tempo.max(20.0).round() as i32;
            s.run_mode = 1; // reloj libre
        }

        let enabled = self.shared.enabled.load(Ordering::Relaxed);
        if !enabled || !transport_playing {
            self.shared.current_step.store(255, Ordering::Relaxed);
            self.shared.current_abs_step.store(u32::MAX, Ordering::Relaxed);
            return out_buf;
        }

        let schedule = self.shared.schedule.lock().unwrap().clone();
        let settings = *self.shared.settings.lock().unwrap();
        // Reloj único: el scheduler deriva su posición del transporte del
        // launcher, no de su contador interno. Así el 1.1 de la batería
        // coincide con el 1.1 de los clips y la frontera de rec_gate.
        let transport = Transport {
            playing: true,
            song_pos_beats: Some(song_beats),
            tempo: Some(tempo),
        };

        let (info, events) = self.inner.process(
            frames as u32,
            &transport,
            &settings,
            &schedule,
            &[],
            &[],
        );

        if info.running && info.step >= 0 {
            self.shared.current_step.store((info.step % 16) as u8, Ordering::Relaxed);
            self.shared.current_abs_step.store(info.step as u32, Ordering::Relaxed);
        } else {
            self.shared.current_step.store(255, Ordering::Relaxed);
            self.shared.current_abs_step.store(u32::MAX, Ordering::Relaxed);
        }

        let mut count = 0;
        for ev in events {
            if let caja_core::scheduler::OutKind::NoteOn { note, vel, .. } = ev.kind {
                if note >= 36 && note < 52 && count < 64 {
                    out_buf[count] = (ev.time, note - 36, vel);
                    count += 1;
                }
            }
        }

        out_buf
    }

    /// Ciclo de Groove en ticks, para alinear lanzamientos de clips.
    pub fn groove_loop_ticks(&self) -> u32 {
        self.shared.groove_loop_ticks.load(Ordering::Relaxed)
    }

    pub fn reset(&mut self) {
        self.inner.reset();
        self.shared.current_step.store(255, Ordering::Relaxed);
        self.shared.current_abs_step.store(u32::MAX, Ordering::Relaxed);
    }
}
