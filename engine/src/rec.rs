//! Grabación MIDI (Fase 3D): el hilo de audio captura las notas en vivo con
//! su posición exacta en ticks dentro del loop y las publica por este anillo
//! lock-free. La UI las drena y las añade al clip (overdub).
//! Mismo patrón que el Spsc de Caja, pero con un POD de nota en vez de u32.
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};

const CAP: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecNote {
    pub track: u8,
    /// Escena del clip que se grababa (el cursor de grabación puede cambiar antes de que
    /// la UI reciba la nota, así que el destino viaja con ella).
    pub scene: u8,
    pub note: u8,
    pub vel: u8,
    pub tick: u32, // posición dentro del loop del clip
    pub dur: u32,  // duración en ticks
}

impl RecNote {
    pub const EMPTY: Self = RecNote { track: 0, scene: 0, note: 0, vel: 0, tick: 0, dur: 0 };
}

/// Un productor (audio) y un consumidor (UI). Sin alloc en push/pop.
pub struct RecRing {
    buf: [UnsafeCell<RecNote>; CAP],
    head: AtomicUsize,
    tail: AtomicUsize,
}

// SAFETY: head solo lo escribe el productor, tail solo el consumidor; cada
// hueco lo escribe uno y lo lee el otro, ordenados por Release/Acquire.
unsafe impl Sync for RecRing {}
unsafe impl Send for RecRing {}

impl RecRing {
    pub fn new() -> Self {
        RecRing {
            buf: std::array::from_fn(|_| UnsafeCell::new(RecNote::EMPTY)),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Hilo de audio. false si está lleno (se descarta la nota, nunca bloquea).
    pub fn push(&self, n: RecNote) -> bool {
        let h = self.head.load(Ordering::Relaxed);
        if h.wrapping_sub(self.tail.load(Ordering::Acquire)) >= CAP { return false; }
        // SAFETY: este hueco es del productor hasta que avance head.
        unsafe { *self.buf[h % CAP].get() = n; }
        self.head.store(h.wrapping_add(1), Ordering::Release);
        true
    }

    /// Hilo de UI.
    pub fn pop(&self) -> Option<RecNote> {
        let t = self.tail.load(Ordering::Relaxed);
        if t == self.head.load(Ordering::Acquire) { return None; }
        // SAFETY: este hueco es del consumidor hasta que avance tail.
        let n = unsafe { *self.buf[t % CAP].get() };
        self.tail.store(t.wrapping_add(1), Ordering::Release);
        Some(n)
    }
}
