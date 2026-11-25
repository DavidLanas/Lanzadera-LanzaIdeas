// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! Motor de drums de Caja integrado en Lanzadera (Fase 4A).
//!
//! Wrapper del Engine de Caja como "pista especial" de drums del launcher.
//! Recibe triggers desde el MK3 (16 pads) o desde el clip de drums y
//! renderiza audio sample-accurate en el mismo callback que clips/CLAP.

pub mod engine;
pub mod state;
pub mod sample;
pub mod store;
pub mod scheduler;
pub mod loader;

pub use engine::CajaEngine;
pub use sample::Sample;
pub use store::CajaStore;
pub use scheduler::{CajaScheduler, CajaSchedulerShared};
pub use loader::CajaLoader;
pub use state::default_kit_state;
pub mod library_state;
