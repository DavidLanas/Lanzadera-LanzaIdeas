// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! Tokens de color — estética DAW oscura para Lanzadera.
//! Fase 1: paleta neutral black-gray inspirada visualmente en Futureboard,
//! acento verde inspirado visualmente en Ponzik. Solo valores visuales;
//! no cambia semántica de estado ni lógica.
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
    pub accent_hover: Color32,
    pub accent_active: Color32,
    pub accent_dim: Color32,
    pub accent_hot: Color32,
    pub accent_on: Color32,
    pub accent_gold: Color32,
    pub accent_green: Color32,
    pub bg: Color32,
    pub panel: Color32,
    pub panel_raised: Color32,
    pub transport_bg: Color32,
}

pub const T: Theme = Theme {
    // Clip states — semántica preservada: play=green, trigger=amber, rec=red.
    clip_empty:     Color32::from_rgb(39, 41, 48),      // neutral raised slot
    clip_playing:   Color32::from_rgb(63, 212, 90),     // green accent/play
    clip_triggered: Color32::from_rgb(240, 190, 55),    // amber
    clip_stopping:  Color32::from_rgb(230, 140, 50),    // orange
    clip_rec:       Color32::from_rgb(220, 55, 55),     // red

    // Mixer / strips
    strip_bg:       Color32::from_rgb(31, 33, 39),
    strip_border:   Color32::from_rgb(56, 59, 68),
    fader_track:    Color32::from_rgb(16, 17, 21),
    fader_thumb:    Color32::from_rgb(224, 226, 232),

    // Meters — semántica preservada
    meter_low:      Color32::from_rgb(63, 212, 90),
    meter_mid:      Color32::from_rgb(230, 200, 60),
    meter_high:     Color32::from_rgb(235, 80, 70),

    // Controls
    knob_bg:        Color32::from_rgb(46, 49, 58),
    knob_arc:       Color32::from_rgb(63, 212, 90),

    // Piano roll
    piano_white:    Color32::from_rgb(236, 237, 240),
    piano_black:    Color32::from_rgb(28, 30, 36),
    piano_note:     Color32::from_rgb(63, 212, 90),
    piano_head:     Color32::from_rgb(194, 255, 185),

    // Text
    text_dim:       Color32::from_rgb(141, 144, 152),
    text_bright:    Color32::from_rgb(236, 237, 240),

    // Accent family — Ponzik-inspired green, reimplemented as original tokens.
    accent:         Color32::from_rgb(63, 212, 90),     // #3FD45A
    accent_hover:   Color32::from_rgb(88, 224, 112),
    accent_active:  Color32::from_rgb(35, 168, 58),     // deep/dim
    accent_dim:     Color32::from_rgb(35, 168, 58),
    accent_hot:     Color32::from_rgb(194, 255, 185),   // glow/highlight
    accent_on:      Color32::from_rgb(4, 35, 11),       // text on accent

    // Legacy semantic accents preserved for existing call sites.
    accent_gold:    Color32::from_rgb(200, 165, 80),
    accent_green:   Color32::from_rgb(63, 212, 90),

    // Surfaces — Futureboard-inspired neutral dark scale.
    bg:             Color32::from_rgb(20, 21, 25),      // near #141519
    panel:          Color32::from_rgb(29, 30, 35),      // near #1D1E23
    panel_raised:   Color32::from_rgb(39, 41, 48),      // near #272830
    transport_bg:   Color32::from_rgb(23, 24, 29),      // near #17181D
};

pub fn apply(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = T.bg;
    v.window_fill = T.panel;
    v.extreme_bg_color = T.fader_track;
    v.faint_bg_color = T.panel_raised;
    v.window_rounding = Rounding::same(8.0);
    v.menu_rounding = Rounding::same(8.0);
    v.widgets.noninteractive.rounding = Rounding::same(5.0);
    v.widgets.inactive.rounding = Rounding::same(5.0);
    v.widgets.hovered.rounding = Rounding::same(5.0);
    v.widgets.active.rounding = Rounding::same(5.0);
    v.widgets.noninteractive.bg_fill = T.panel;
    v.widgets.noninteractive.fg_stroke.color = T.text_bright;
    v.widgets.inactive.bg_fill = T.panel_raised;
    v.widgets.inactive.fg_stroke.color = T.text_bright;
    v.widgets.hovered.bg_fill = Color32::from_rgb(52, 56, 66);
    v.widgets.hovered.fg_stroke.color = T.text_bright;
    v.widgets.active.bg_fill = Color32::from_rgb(45, 72, 52);
    v.widgets.active.fg_stroke.color = T.text_bright;
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(63, 212, 90, 45);
    v.selection.stroke.color = T.accent;
    v.hyperlink_color = T.accent;
    v.warn_fg_color = Color32::from_rgb(240, 190, 55);
    v.error_fg_color = Color32::from_rgb(235, 80, 70);
    ctx.set_visuals(v);

    let mut s = (*ctx.style()).clone();
    s.spacing.item_spacing = vec2(6.0, 5.0);
    s.spacing.button_padding = vec2(8.0, 4.0);
    s.spacing.indent = 12.0;
    s.spacing.interact_size.y = 24.0;
    ctx.set_style(s);
}
