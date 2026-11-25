// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! Almacén de muestras del motor de drums.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::Arc;
use std::cell::UnsafeCell;

use caja_core::kit::NUM_PADS;
use caja_core::library::MAX_BANK;

use crate::caja_drums::sample::Sample;
use crate::caja_drums::loader::RequestQueue;

const TOTAL_SLOTS: usize = NUM_PADS * MAX_BANK;

pub struct CajaStore {
    pub slots: Vec<UnsafeCell<Arc<Option<Arc<Sample>>>>>,
    pub bank_len: [AtomicU64; NUM_PADS],
    pub cur_idx: [AtomicI32; NUM_PADS],
    pub pin_idx: [AtomicI32; NUM_PADS],
    pub pin_missing: [AtomicBool; NUM_PADS],
    pub requested: [AtomicBool; TOTAL_SLOTS],
    pub last_use: [AtomicU64; TOTAL_SLOTS],
    pub clock: AtomicU64,
    pub want: RequestQueue,
}

// SAFETY: el loader escribe, el audio lee, pero nunca concurrentemente en la misma ranura.
unsafe impl Sync for CajaStore {}

impl CajaStore {
    pub fn new() -> Self {
        CajaStore {
            slots: (0..TOTAL_SLOTS)
                .map(|_| UnsafeCell::new(Arc::new(None)))
                .collect(),
            bank_len: std::array::from_fn(|_| AtomicU64::new(0)),
            cur_idx: std::array::from_fn(|_| AtomicI32::new(-1)),
            pin_idx: std::array::from_fn(|_| AtomicI32::new(-1)),
            pin_missing: std::array::from_fn(|_| AtomicBool::new(false)),
            requested: std::array::from_fn(|_| AtomicBool::new(false)),
            last_use: std::array::from_fn(|_| AtomicU64::new(0)),
            clock: AtomicU64::new(0),
            want: RequestQueue::new(),
        }
    }

    pub fn slot(pad: usize, idx: usize) -> usize { pad * MAX_BANK + idx }

    /// Lee la muestra en la ranura (audio thread).
    pub fn load_sample(&self, pad: usize, idx: usize) -> Option<Arc<Sample>> {
        let slot = Self::slot(pad, idx);
        // SAFETY: solo lectura, sin race con el loader (que escribe en otra ranura o ya terminó).
        unsafe { (*self.slots[slot].get()).as_ref().clone() }
    }

    /// Reemplaza la muestra en la ranura (loader thread).
    pub fn store_sample(&self, pad: usize, idx: usize, s: Arc<Sample>) {
        let slot = Self::slot(pad, idx);
        let new = Arc::new(Some(s));
        // SAFETY: el loader es el único escritor; el audio solo lee.
        unsafe { *self.slots[slot].get() = new; }
        self.bank_len[pad].fetch_max((idx + 1) as u64, Ordering::Relaxed);
    }

    pub fn set_bank_len(&self, pad: usize, len: usize) {
        self.bank_len[pad].store(len.min(MAX_BANK) as u64, Ordering::Relaxed);
    }
}
