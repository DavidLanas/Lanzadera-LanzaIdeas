// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! lanzadera-ui — widgets egui del launcher (Fase 3A).
pub mod theme;
pub mod widgets;
pub mod piano_roll;
pub mod eq_view;
pub mod view;

pub use view::SessionView;
pub use piano_roll::PianoRoll;
/// Ventanas flotantes de parámetros de plugin (Fase 3H).
pub mod plugin_window;

// ── Patches Lanzadera ──
pub mod patches;
pub mod waveform;
