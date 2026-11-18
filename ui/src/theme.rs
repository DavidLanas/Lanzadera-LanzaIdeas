//! Tokens de color — estética Lemon Studio: oscuro suave, bordes redondeados,
//! fuente Ubuntu (la que egui trae por defecto). Hereda el espíritu de theme.rs
//! de Caja pero sin el look pixelado.
use egui::{Color32, Rounding, vec2};

pub struct Theme {
    pub clip_empty: Color32,
    pub clip_playing: Color32,
    pub clip_triggered: Color32,
    pub clip_stopping: Color32,
    pub clip_rec: Color32,
    pub strip_bg: Color32,
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
    pub accent: Color32,
    pub bg: Color32,
    pub panel: Color32,
}

pub const T: Theme = Theme {
    clip_empty:     Color32::from_rgb(38, 40, 46),
    clip_playing:   Color32::from_rgb(80, 220, 120),
    clip_triggered: Color32::from_rgb(240, 200, 60),
    clip_stopping:  Color32::from_rgb(240, 140, 50),
    clip_rec:       Color32::from_rgb(230, 60, 60),
    strip_bg:       Color32::from_rgb(30, 32, 38),
    fader_track:    Color32::from_rgb(22, 24, 28),
    fader_thumb:    Color32::from_rgb(200, 205, 215),
    meter_low:      Color32::from_rgb(70, 200, 110),
    meter_mid:      Color32::from_rgb(230, 200, 60),
    meter_high:     Color32::from_rgb(235, 80, 70),
    knob_bg:        Color32::from_rgb(44, 47, 54),
    knob_arc:       Color32::from_rgb(90, 170, 235),
    piano_white:    Color32::from_rgb(225, 227, 232),
    piano_black:    Color32::from_rgb(40, 42, 48),
    piano_note:     Color32::from_rgb(90, 170, 235),
    piano_head:     Color32::from_rgb(240, 200, 60),
    text_dim:       Color32::from_gray(150),
    accent:         Color32::from_rgb(90, 170, 235),
    bg:             Color32::from_rgb(24, 26, 30),
    panel:          Color32::from_rgb(33, 36, 42),
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
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(90, 170, 235, 60);
    ctx.set_visuals(v);
    let mut s = (*ctx.style()).clone();
    s.spacing.item_spacing = vec2(6.0, 6.0);
    s.spacing.button_padding = vec2(6.0, 3.0);
    ctx.set_style(s);
}
