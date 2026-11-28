// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! Tokens visuales del Clip Launch / grid.
//! Fase 1: neutral dark + acento verde, inspirados visualmente en
//! Futureboard/Ponzik. Reimplementación original; sin cambios de lógica.
use egui::Color32;

// Grid / surfaces
pub const GRID_LINE: Color32 = Color32::from_rgb(52, 55, 64);
pub const TRACK_HEADER_BG: Color32 = Color32::from_rgb(31, 33, 39);
pub const SCENE_COL_BG: Color32 = Color32::from_rgb(21, 23, 27);
pub const CLIP_BORDER: Color32 = Color32::from_rgb(66, 70, 80);

// Meters / progress
pub const METER_BG: Color32 = Color32::from_rgb(20, 21, 25);
pub const METER_FILL: Color32 = Color32::from_rgb(63, 212, 90);
pub const PROGRESS_FILL: Color32 = Color32::from_rgb(63, 212, 90);

// Clip background states — semántica preservada
pub const CLIP_BG_PLAYING: Color32 = Color32::from_rgb(42, 78, 52);
pub const CLIP_BG_TRIGGERED: Color32 = Color32::from_rgb(82, 74, 34);
pub const CLIP_BG_RECORDING: Color32 = Color32::from_rgb(110, 42, 42);
pub const CLIP_BG_STOPPED: Color32 = Color32::from_rgb(38, 41, 48);

// Text / semantic
pub const BADGE_TEXT: Color32 = Color32::from_rgb(141, 144, 152);
pub const TEXT_COLOR: Color32 = Color32::from_rgb(236, 237, 240);
pub const RED: Color32 = Color32::from_rgb(220, 60, 60);
pub const GRAY: Color32 = Color32::from_rgb(88, 91, 99);

// Layout (unchanged)
pub const HEADER_H: f32 = 48.0;
pub const MIN_CELL_W: f32 = 100.0;
pub const MAX_CELL_W: f32 = 200.0;
pub const MIN_CELL_H: f32 = 40.0;
pub const MAX_CELL_H: f32 = 80.0;
pub const TRACK_HEADER_W: f32 = 160.0;
pub const FONT_SMALL: f32 = 10.0;
pub const FONT_BASE: f32 = 11.0;
pub const FONT_MEDIUM: f32 = 12.0;
pub const FONT_LARGE: f32 = 14.0;
