//! Grabacion de audio a clip (Fase F3/F4).
//!
//! Un UNICO buffer activo: `audio.recording` es global con un solo
//! (rec_track, rec_scene), asi que solo una pista graba a la vez. Reservar
//! un buffer por (pista x escena) inflaba la memoria virtual sin motivo.
//!
//! El buffer se preasigna en el hilo principal. Al detener se entrega al
//! loader intercambiandolo con un spare (mover un Vec son 3 punteros, sin
//! asignar). El loader recicla el Vec ya usado para que la siguiente
//! grabacion no asigne memoria en el callback RT.
//!
//! Handoff RT->UI inspirado en Maolan (BSD-2). Conceptos de sesion/pista
//! de MAGDA (GPL-3). API de escritura WAV: hound (MPL-2.0).

use std::sync::Mutex;

/// Buffers ya usados, devueltos por el loader para reutilizar sin asignar en RT.
static REC_RECYCLE: Mutex<Vec<Vec<f32>>> = Mutex::new(Vec::new());

/// Devuelve un buffer al pool. Se llama desde el loader, NUNCA desde RT.
pub fn recycle_buffer(mut buf: Vec<f32>) {
    buf.clear();
    if let Ok(mut pool) = REC_RECYCLE.lock() {
        if pool.len() < 2 {
            pool.push(buf);
        }
    }
}

/// Toma un buffer reciclado con capacidad suficiente. `try_lock`: si el
/// loader esta ocupado, no bloqueamos el callback de audio.
fn take_recycled(min_cap: usize) -> Option<Vec<f32>> {
    let mut pool = REC_RECYCLE.try_lock().ok()?;
    while let Some(v) = pool.pop() {
        if v.capacity() >= min_cap {
            return Some(v);
        }
    }
    None
}

/// Audio grabado, listo para que el loader lo escriba a WAV.
#[derive(Clone)]
pub struct RecordedClip {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
}

struct Active {
    track: usize,
    scene: usize,
    channels: u16,
    sample_rate: u32,
}

pub struct RecordEngine {
    /// Buffer que se esta escribiendo.
    buf: Vec<f32>,
    /// Reemplazo para el intercambio al detener.
    spare: Vec<f32>,
    capacity: usize,
    active: Option<Active>,
}

impl RecordEngine {
    /// `seconds` = duracion maxima de una grabacion (estereo como cota).
    pub fn new(sample_rate: u32, seconds: usize) -> Self {
        let capacity = (sample_rate as usize) * seconds * 2;
        println!(
            "[record] buffer de grabacion: {} s estereo a {} Hz ({:.1} MB x2)",
            seconds,
            sample_rate,
            (capacity * 4) as f64 / 1_048_576.0
        );
        RecordEngine {
            buf: Vec::with_capacity(capacity),
            spare: Vec::with_capacity(capacity),
            capacity,
            active: None,
        }
    }

    /// Arma la grabacion. Si no hay buffer con capacidad, se rechaza con
    /// aviso: nunca se asigna memoria en el callback de audio.
    pub fn start_recording(&mut self, track: usize, scene: usize, channels: u16, sample_rate: u32) {
        if self.active.is_some() {
            return;
        }
        if self.buf.capacity() < self.capacity {
            if let Some(v) = take_recycled(self.capacity) {
                self.buf = v;
            }
        }
        if self.buf.capacity() == 0 {
            eprintln!("[record] sin buffer libre: espera a que se guarde la grabacion anterior");
            return;
        }
        self.buf.clear();
        self.active = Some(Active { track, scene, channels, sample_rate });
    }

    /// Detiene y entrega el audio. Intercambia el buffer por el spare para
    /// poder seguir grabando sin asignar en RT.
    pub fn stop_recording(&mut self, track: usize, scene: usize) -> Option<RecordedClip> {
        let a = self.active.take()?;
        if a.track != track || a.scene != scene {
            self.active = Some(a);
            return None;
        }

        let replacement = if self.spare.capacity() >= self.capacity {
            std::mem::take(&mut self.spare)
        } else if let Some(v) = take_recycled(self.capacity) {
            v
        } else {
            Vec::new()
        };

        let filled = std::mem::replace(&mut self.buf, replacement);
        Some(RecordedClip {
            samples: filled,
            sample_rate: a.sample_rate,
            channels: a.channels,
        })
    }

    /// Escribe muestras. Nunca realloca: si el buffer se llena, trunca.
    pub fn write(&mut self, data: &[f32]) {
        if self.active.is_none() {
            return;
        }
        let room = self.buf.capacity().saturating_sub(self.buf.len());
        if room == 0 {
            return;
        }
        let take = data.len().min(room);
        self.buf.extend_from_slice(&data[..take]);
    }

    pub fn is_any_recording(&self) -> bool {
        self.active.is_some()
    }
}
