// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! Triple buffer sin bloqueos — port textual de caja/core/src/schedule.rs
//! (sin el Schedule de golpes, que es específico de Caja).
//! Un productor a la vez (serializado por un Mutex SOLO en su lado) y UN
//! consumidor (el hilo de audio) que nunca bloquea.
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

const NEW: usize = 4;

pub struct TripleBuffer<T> {
    slots: [UnsafeCell<T>; 3],
    /// Índice del hueco listo para leer, con el bit NEW si hay datos sin consumir.
    ready: AtomicUsize,
    /// Hueco de escritura (solo lo toca el productor, bajo el mutex).
    write: Mutex<usize>,
    /// Hueco de lectura (solo lo toca el consumidor).
    read: AtomicUsize,
}

// SAFETY: los tres índices (write, ready, read) son siempre una permutación de {0,1,2}. El productor
// solo escribe en `write`, el consumidor solo lee `read`, y el intercambio se hace con `swap` atómico
// (AcqRel), así ningún hueco lo usan los dos a la vez. Requiere un único consumidor.
unsafe impl<T: Send> Sync for TripleBuffer<T> {}
unsafe impl<T: Send> Send for TripleBuffer<T> {}

impl<T: Default> Default for TripleBuffer<T> {
    fn default() -> Self { TripleBuffer::new(T::default) }
}

impl<T> TripleBuffer<T> {
    pub fn new(make: impl Fn() -> T) -> Self {
        TripleBuffer {
            slots: [UnsafeCell::new(make()), UnsafeCell::new(make()), UnsafeCell::new(make())],
            ready: AtomicUsize::new(1),
            write: Mutex::new(0),
            read: AtomicUsize::new(2),
        }
    }

    /// Hilo principal / GUI / tarea de fondo: rellena el hueco de escritura y lo publica.
    pub fn publish(&self, fill: impl FnOnce(&mut T)) {
        let mut w = self.write.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: `*w` es del productor (ver arriba).
        fill(unsafe { &mut *self.slots[*w].get() });
        let old = self.ready.swap(*w | NEW, Ordering::AcqRel);
        *w = old & 3;
    }

    /// Hilo de audio (único consumidor). Sin bloqueos ni reservas. La referencia
    /// vale hasta la siguiente llamada a `latest`.
    pub fn latest(&self) -> &T {
        let mut r = self.read.load(Ordering::Relaxed);
        if self.ready.load(Ordering::Acquire) & NEW != 0 {
            let old = self.ready.swap(r, Ordering::AcqRel);
            r = old & 3;
            self.read.store(r, Ordering::Relaxed);
        }
        // SAFETY: `r` es del consumidor (ver arriba).
        unsafe { &*self.slots[r].get() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    #[derive(Clone)]
    struct Big { a: [u64; 64] }
    impl Default for Big { fn default() -> Self { Big { a: [0; 64] } } }

    #[test]
    fn publish_then_latest_sees_new_data() {
        let tb: TripleBuffer<Big> = TripleBuffer::default();
        assert_eq!(tb.latest().a[0], 0);
        tb.publish(|b| b.a = [7; 64]);
        assert_eq!(tb.latest().a[63], 7);
        tb.publish(|b| b.a = [8; 64]);
        tb.publish(|b| b.a = [9; 64]);
        assert_eq!(tb.latest().a[0], 9); // se queda con lo último
        assert_eq!(tb.latest().a[0], 9);
    }

    #[test]
    fn concurrent_snapshots_are_never_torn_and_never_go_backwards() {
        let tb = Arc::new(TripleBuffer::<Big>::default());
        let stop = Arc::new(AtomicBool::new(false));
        let producer = {
            let (tb, stop) = (tb.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut n = 1u64;
                while !stop.load(Ordering::Relaxed) {
                    tb.publish(|b| b.a = [n; 64]);
                    n += 1;
                }
                n
            })
        };
        let mut last = 0u64;
        let mut reads = 0u64;
        while last < 20_000 && reads < 200_000_000 {
            let s = tb.latest();
            let first = s.a[0];
            assert!(s.a.iter().all(|&x| x == first), "lectura rota");
            assert!(first >= last, "retrocedió");
            last = first;
            reads += 1;
        }
        assert!(last >= 20_000, "el productor no avanzó");
        stop.store(true, Ordering::Relaxed);
        assert!(producer.join().unwrap() > 1);
    }
}
