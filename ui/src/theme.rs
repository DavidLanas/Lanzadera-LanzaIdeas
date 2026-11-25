// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! Tokens de color — estética DAW oscura para Lanzadera (clip launcher live).
//! Inspiración: consolas analógicas + Lemon Studio / Bitwig-like dark panels.
//! Paleta: charcoal / near-black, cream foreground, amber, phosphor green, vintage gold.
use egui::{Color32, Rounding, vec2};

pub struct Theme {
    pub clip_empty: Color32,
    pub clip_playing: Color32,
    pub clip_triggered: Color32,
    pub clip_stopping: Color32,
    pub clip_rec: Color32,
    pub strip_bg: Color32,
    pub strip_border: Color32,
    pub fader_track: Color32,
    pub fader_thumb: Color32,
    pub meter_low: Color32,
    pub meter_mid: Color32,
    pub meter_high: Color32,
    pub knob_bg: Color32,
    pub knob_arc: Color32,
    pub piano_white: Color32,
    pub piano_black: Color32,
    pub piano_note: Color32,
    pub piano_head: Color32,
    pub text_dim: Color32,
    pub text_bright: Color32,
    pub accent: Color32,
    pub accent_gold: Color32,
    pub accent_green: Color32,
    pub bg: Color32,
    pub panel: Color32,
    pub panel_raised: Color32,
    pub transport_bg: Color32,
}

pub const T: Theme = Theme {
    clip_empty:     Color32::from_rgb(36, 38, 44),
    clip_playing:   Color32::from_rgb(72, 210, 130),   // phosphor green
    clip_triggered: Color32::from_rgb(240, 190, 55),   // amber
    clip_stopping:  Color32::from_rgb(230, 140, 50),
    clip_rec:       Color32::from_rgb(220, 55, 55),
    strip_bg:       Color32::from_rgb(28, 30, 36),
    strip_border:   Color32::from_rgb(48, 52, 60),
    fader_track:    Color32::from_rgb(18, 20, 24),
    fader_thumb:    Color32::from_rgb(210, 205, 195),  // cream
    meter_low:      Color32::from_rgb(70, 200, 110),
    meter_mid:      Color32::from_rgb(230, 200, 60),
    meter_high:     Color32::from_rgb(235, 80, 70),
    knob_bg:        Color32::from_rgb(42, 45, 52),
    knob_arc:       Color32::from_rgb(90, 170, 235),
    piano_white:    Color32::from_rgb(230, 225, 215),
    piano_black:    Color32::from_rgb(32, 34, 40),
    piano_note:     Color32::from_rgb(90, 170, 235),
    piano_head:     Color32::from_rgb(240, 190, 55),
    text_dim:       Color32::from_rgb(140, 145, 155),
    text_bright:    Color32::from_rgb(230, 225, 215),
    accent:         Color32::from_rgb(90, 170, 235),
    accent_gold:    Color32::from_rgb(200, 165, 80),   // vintage amp gold
    accent_green:   Color32::from_rgb(72, 210, 130),
    bg:             Color32::from_rgb(20, 22, 26),
    panel:          Color32::from_rgb(30, 33, 39),
    panel_raised:   Color32::from_rgb(38, 42, 50),
    transport_bg:   Color32::from_rgb(24, 26, 32),
};

pub fn apply(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = T.bg;
    v.window_fill = T.panel;
    v.extreme_bg_color = T.fader_track;
    v.window_rounding = Rounding::same(6.0);
    v.menu_rounding = Rounding::same(6.0);
    v.widgets.noninteractive.rounding = Rounding::same(4.0);
    v.widgets.inactive.rounding = Rounding::same(4.0);
    v.widgets.hovered.rounding = Rounding::same(4.0);
    v.widgets.active.rounding = Rounding::same(4.0);
    v.widgets.noninteractive.bg_fill = T.panel;
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(90, 170, 235, 55);
    v.widgets.inactive.bg_fill = T.panel_raised;
    v.widgets.hovered.bg_fill = Color32::from_rgb(48, 54, 64);
    v.widgets.active.bg_fill = Color32::from_rgb(55, 62, 74);
    ctx.set_visuals(v);
    let mut s = (*ctx.style()).clone();
    s.spacing.item_spacing = vec2(6.0, 5.0);
    s.spacing.button_padding = vec2(7.0, 3.0);
    s.spacing.indent = 12.0;
    ctx.set_style(s);
}
