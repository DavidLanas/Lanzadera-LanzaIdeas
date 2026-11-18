//! Feedback de LEDs del MK3 — protocolo del script Bitwig v4.8.
//!
//! Los 41 botones (CC 20..60) van a DIM=42 en reposo, ON=127 si su función
//! está activa, y ON inmediato mientras están pulsados (bitmask mk3_buttons).
//!
//! Los pads NO se reconstruyen desde el Schedule: la UI publica máscaras
//! exactas (gm_step_bits / gm_hit_bits / gm_playhead / groove_rack_bits),
//! así el LED siempre coincide con lo que se ve en la GUI.
//! Este hilo es normal (puede usar Mutex); el de audio no lo toca.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use midir::{MidiOutput, MidiOutputConnection};
use lanzadera_core::ClipState;
use lanzadera_engine::shared::SessionShared;

use crate::mk3::{led, PAD_NOTES};

const DIM: u8 = 42;
const ON: u8 = 127;
/// Mitad de brillo: los cuatro botones globales de CHORDS están vivos.
const SOFT: u8 = 84;
const CC_LO: u8 = 20;
const CC_HI: u8 = 60;
const N_BTNS: usize = 41;

const C_MASCHINE: u8 = 20;
const C_PITCH: u8 = 30;
const C_MOD: u8 = 31;
const C_PERFORM: u8 = 32;
const C_NOTES: u8 = 33;
const C_VOLUME: u8 = 23;
const C_SWING: u8 = 24;
const C_TEMPO: u8 = 25;
const C_GROUP: u8 = 34;
const C_AUTO: u8 = 35;
const C_LOCK: u8 = 36;
const C_NOTE_REPEAT: u8 = 37;
const C_TAP: u8 = 40;
const C_PLAY: u8 = 42;
const C_REC: u8 = 43;
const C_FIXED_VEL: u8 = 46;
const C_PAD_MODE: u8 = 47;
const C_KEYBOARD: u8 = 48;
const C_CHORDS: u8 = 49;
const C_STEP: u8 = 50;
const C_PATTERN: u8 = 52;
const C_EVENTS: u8 = 53;

/// Color de reposo de una celda vacía en modo Step (amarillo pálido).
const STEP_IDLE: u8 = 22;
/// Paleta por pad para el modo PAD (ritmo del beat).
const PAD_HUE: [u8; 8] = [
    led::RED, led::ORANGE, led::WARM_YELLOW, led::LIME,
    led::MINT, led::CYAN, led::VIOLET, led::MAGENTA,
];

pub struct LedHandle { stop: Arc<AtomicBool> }
impl LedHandle { pub fn stop(&self) { self.stop.store(true, Ordering::Relaxed); } }
impl Drop for LedHandle { fn drop(&mut self) { self.stop(); } }

pub fn rgb_to_pad_color(rgb: [u8; 3]) -> u8 {
    let (r, g, b) = (rgb[0] as i32, rgb[1] as i32, rgb[2] as i32);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let diff = max - min;
    let sat = if max == 0 { 0.0 } else { diff as f32 / max as f32 };
    if sat < 0.2 { return led::WHITE; }
    let mut hue = 0.0f32;
    if diff != 0 {
        let (rf, gf, bf, df) = (r as f32, g as f32, b as f32, diff as f32);
        if max == r      { hue = 60.0 * (((gf - bf) / df) % 6.0); }
        else if max == g { hue = 60.0 * (((bf - rf) / df) + 2.0); }
        else             { hue = 60.0 * (((rf - gf) / df) + 4.0); }
    }
    if hue < 0.0 { hue += 360.0; }
    match hue as i32 {
        0..=14 | 345..=360 => led::RED,
        15..=29  => led::ORANGE,
        30..=44  => led::WARM_YELLOW,
        45..=69  => led::YELLOW,
        70..=89  => led::LIME,
        90..=149 => led::GREEN,
        150..=164 => led::MINT,
        165..=179 => led::CYAN,
        180..=194 => led::TURQUOISE,
        195..=239 => led::BLUE,
        240..=269 => led::VIOLET,
        270..=299 => led::PURPLE,
        300..=319 => led::MAGENTA,
        _ => led::FUCHSIA,
    }
}

pub fn spawn_mk3_leds(name_hint: &str, shared: Arc<SessionShared>) -> Result<LedHandle, String> {
    let out = MidiOutput::new("lanzadera-leds").map_err(|e| e.to_string())?;
    let port = out.ports().into_iter()
        .find(|p| out.port_name(p).map(|n| n.contains(name_hint)).unwrap_or(false))
        .ok_or_else(|| format!("puerto de salida '{}' no encontrado", name_hint))?;
    let mut conn: MidiOutputConnection = out.connect(&port, "lanzadera-mk3-leds").map_err(|e| e.to_string())?;

    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    std::thread::Builder::new().name("lanzadera-leds".into()).spawn(move || {
        let mut last_pads = [0xFFu8; 16];
        let mut last_btns = [0xFFu8; N_BTNS];
        // Diagnóstico: en reposo los botones no deberían cambiar. Si un
        // estado oscila, aquí queda registrado qué CC es el culpable.
        let mut btn_flips = [0u16; N_BTNS];
        let mut last_report = std::time::Instant::now();

        while !stop2.load(Ordering::Relaxed) {
            let (cursor, playing, recording, metronome, exists, base) = {
                let st = shared.state.lock().unwrap();
                let c = st.selected_track.min(st.tracks.len().saturating_sub(1));
                let tr = &st.tracks[c];
                let mut ex = [false; 16];
                for i in 0..16 { ex[i] = tr.clips.get(i).map(|o| o.is_some()).unwrap_or(false); }
                (c, st.playing, st.recording, st.metronome, ex, rgb_to_pad_color(tr.color))
            };

            let gm        = shared.groove_machine_visible.load(Ordering::Relaxed);
            let sub       = shared.groove_sub_mode.load(Ordering::Relaxed);
            let buttons   = shared.mk3_buttons.load(Ordering::Relaxed);
            let flags     = shared.mk3_flags.load(Ordering::Relaxed);
            let wins      = shared.window_flags.load(Ordering::Relaxed);
            let ev        = shared.mk3_event_mode.load(Ordering::Relaxed);
            let follow   = shared.mk3_follow_playhead.load(Ordering::Relaxed);
            let pat       = shared.mk3_pattern_mode.load(Ordering::Relaxed);
            let preset_mode = shared.mk3_preset_mode.load(Ordering::Relaxed);
            let (preset_active, preset_slots) = {
                let state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                (
                    state.groove_preset_active.min(7),
                    state.groove_preset_slots.clone(),
                )
            };
            let rack_bits = shared.groove_rack_bits.load(Ordering::Relaxed);
            let active    = shared.groove_active_pat.load(Ordering::Relaxed) as usize;
            let step_bits = shared.gm_step_bits.load(Ordering::Relaxed);
            let soft_bits = shared.gm_step_soft_bits.load(Ordering::Relaxed);
            let hit_bits  = shared.gm_hit_bits.load(Ordering::Relaxed);
            let head      = shared.gm_playhead.load(Ordering::Relaxed);
            let sel_piece = shared.groove_sel_piece.load(Ordering::Relaxed) as usize;
            let sel_pad = shared.groove_sel_pad.load(Ordering::Relaxed) as usize;
            let live = shared.kit.snapshot_live();
            let sel_mode = live.get(sel_pad.min(24)).map(|l| l.mode).unwrap_or(0);
            let chords = gm && shared.mk3_chords_mode.load(Ordering::Relaxed);
            let chords_param = shared.mk3_chords_param.load(Ordering::Relaxed);
            // CHORDS tiene prioridad absoluta: se neutraliza el resto de
            // sub-modos para que ningún valor transitorio de groove_sub_mode
            // pueda alternar colores ni estados de botón (fin del pestañeo).
            let sub = if chords { 3u8 } else { sub };
            let ev = if chords { false } else { ev };
            let pat = if chords { false } else { pat };
            let preset_mode = if chords { false } else { preset_mode };

            // ── PADS ──────────────────────────────────────────────────
            for i in 0..16 {
                let col = if gm && preset_mode {
                    if i < 8 && preset_slots[i].is_some() {
                        if i == preset_active { led::CYAN } else { led::BLUE }
                    } else {
                        led::DIM
                    }
                } else if gm && pat {
                    // PATTERN: pads = slots del banco del patrón activo
                    let slot = (active / 16) * 16 + i;
                    if slot == active { led::WHITE }
                    else if rack_bits & (1 << slot) != 0 { led::VIOLET }
                    else { led::DIM }
                } else if gm && (ev || sub == 1) {
                    // EVENT / STEP: 16 pasos de la celda elegida + playhead con estela
                    let on    = step_bits & (1 << i) != 0;
                    let ghost = step_bits & (1 << (i + 16)) != 0;
                    let soft  = soft_bits & (1 << i) != 0;
                    let is_head = head != 255 && head as usize == i;
                    let is_tail = head != 255 && ((head as usize + 15) % 16) == i;
                    if is_head { led::WHITE }
                    else if ghost { led::CYAN }
                    else if on && soft { led::TURQUOISE }
                    else if on && is_tail { led::WARM_YELLOW }
                    else if on { led::BLUE }
                    else if is_tail { led::ORANGE }
                    else { STEP_IDLE }
                } else if gm && sub == 0 {
                    // PAD: se iluminan al ritmo del beat (pieza con hit en el paso actual)
                    let sounding = hit_bits & (1 << i) != 0;
                    if sounding { PAD_HUE[i % PAD_HUE.len()] }
                    else if head != 255 && head as usize == i { led::DIM }
                    else { led::OFF }
                } else if gm && sub == 2 {
                    // MIXER: fila 0 selección · 1 mute · 2 solo · 3 arm
                    let sel = sel_piece == i;
                    match i / 4 {
                        0 => if sel { led::WHITE } else { led::CYAN },
                        1 => led::ORANGE,
                        2 => led::YELLOW,
                        _ => led::RED,
                    }
                } else if gm && sub == 3 {
                    // CHORDS: celdas del kit. Blanca = seleccionada ·
                    // verde = muestra fijada · cian = algún lock activo ·
                    // paleta = tiene muestras · apagada = vacía.
                    let mode = live.get(i).map(|l| l.mode).unwrap_or(0);
                    let pinned = shared.caja_store.pin_idx[i].load(Ordering::Relaxed) >= 0;
                    let has = shared.caja_store.bank_len[i].load(Ordering::Relaxed) > 0;
                    if sel_pad == i { led::WHITE }
                    else if pinned { led::GREEN }
                    else if mode != 0 { led::CYAN }
                    else if has { PAD_HUE[i % PAD_HUE.len()] }
                    else { led::OFF }
                } else if gm {
                    led::TURQUOISE   // Samples
                } else {
                    let mut c = match shared.rt.clip_state(cursor, i) {
                        ClipState::Playing => led::GREEN,
                        ClipState::Triggered => led::YELLOW,
                        ClipState::RecQueued => led::MAGENTA,
                        ClipState::Recording | ClipState::Overdubbing => led::RED,
                        ClipState::Stopping => led::ORANGE,
                        ClipState::Empty => if exists[i] { base } else { led::OFF },
                    };
                    if head != 255 && head as usize == i { c = led::WHITE; }
                    c
                };

                if col != last_pads[i] {
                    let msg = if col > 0 { [0x90, PAD_NOTES[i], col] } else { [0x80, PAD_NOTES[i], 0] };
                    let _ = conn.send(&msg);
                    last_pads[i] = col;
                }
            }

            // ── BOTONES ───────────────────────────────────────────────
            let win  = |b: u8| if wins  & (1 << b) != 0 { ON } else { DIM };
            let flag = |b: u8| if flags & (1 << b) != 0 { ON } else { DIM };
            let mode = |m: u8| if gm && sub == m && !ev && !pat { ON } else { DIM };
            // Locks de la celda seleccionada: brillante = bloqueado.
            // brillante = bloqueado · medio = libre+encoder · tenue = libre
            let lock_led = |k: u32| {
                if sel_mode >> k & 1 == 1 { ON }
                else if chords_param == k as u8 { SOFT }
                else { DIM }
            };

            for cc in CC_LO..=CC_HI {
                let idx = (cc - CC_LO) as usize;
                let rest = match cc {
                    C_MASCHINE    => if gm { ON } else { DIM },
                    C_VOLUME      => flag(1),
                    C_SWING       => flag(2),
                    C_TEMPO       => flag(3),
                    C_GROUP       => if chords { lock_led(0) } else { win(2) },
                    C_AUTO        => if chords { lock_led(1) } else { win(0) },
                    C_LOCK        => if chords { lock_led(2) } else { win(3) },
                    C_NOTE_REPEAT => if chords { lock_led(3) } else { win(4) },
                    C_PITCH | C_MOD | C_PERFORM | C_NOTES => if chords { SOFT } else { DIM },
                    C_TAP         => if metronome { ON } else { DIM },
                    C_PLAY        => if playing { ON } else { DIM },
                    C_REC         => if recording { ON } else { DIM },
                    C_FIXED_VEL   => flag(0),
                    C_PAD_MODE    => mode(0),
                    C_STEP        => if gm && (sub == 1 || ev) && !pat { ON } else { DIM },
                    C_KEYBOARD    => mode(2),
                    C_CHORDS      => mode(3),
                    C_PATTERN     => if pat { ON } else if gm { DIM } else { ON },
                    C_EVENTS      => if ev && follow { ON } else { DIM },
                    _ => DIM,
                };
                let pressed = (buttons >> idx) & 1 == 1;
                let v = if pressed { ON } else { rest };
                if last_btns[idx] != v {
                    let _ = conn.send(&[0xB0, cc, v]);
                    last_btns[idx] = v;
                    btn_flips[idx] = btn_flips[idx].saturating_add(1);
                }
            }

            if last_report.elapsed() >= std::time::Duration::from_secs(1) {
                let total: u32 = btn_flips.iter().map(|&n| u32::from(n)).sum();
                if total > 20 {
                    let hot: Vec<String> = btn_flips.iter().enumerate()
                        .filter(|(_, &n)| n >= 5)
                        .map(|(i, &n)| format!("cc{}:{}", usize::from(CC_LO) + i, n))
                        .collect();
                    println!("[LED] aviso: {} cambios de botón/s — oscilación en {:?}", total, hot);
                }
                btn_flips = [0u16; N_BTNS];
                last_report = std::time::Instant::now();
            }

            std::thread::sleep(std::time::Duration::from_millis(30));
        }

        for i in 0..16 { let _ = conn.send(&[0x80, PAD_NOTES[i], 0]); }
        for cc in CC_LO..=CC_HI { let _ = conn.send(&[0xB0, cc, 0]); }
    }).map_err(|e| e.to_string())?;
    Ok(LedHandle { stop })
}
