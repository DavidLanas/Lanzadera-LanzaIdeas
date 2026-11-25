// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! Feedback del KeyLab Essential 49: LCD (SysEx) + LEDs de transporte + pads.
//!
//! El LCD usa los dos protocolos del script device_keylab_essential_49.py
//! (ARTURIA y MCU), igual que allí: se envían ambos y el que la unidad
//! entienda es el que se ve. Solo se transmite cuando el texto cambia.
//!
//! Se activa con `--keylab-leds`. Este hilo es normal: puede usar Mutex.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use midir::MidiOutput;
use lanzadera_core::ClipState;
use lanzadera_engine::shared::SessionShared;

use crate::keylab::PAD_NOTES;

/// Notas de los botones de transporte (protocolo Mackie del script .py).
const NOTE_PLAY: u8 = 94;
const NOTE_STOP: u8 = 93;
const NOTE_REC: u8 = 95;
const NOTE_METRO: u8 = 89;

const LCD_COLS: usize = 16;

/// Estado del KeyLab publicado por el hilo de controlador.
/// Un solo KeyLab por sesión: un global del crate es suficiente y evita
/// tocar SessionShared. El hilo de LEDs solo lo lee.
pub static KL_MODE: AtomicU8 = AtomicU8::new(0);
pub static KL_PADS_BANK: AtomicU8 = AtomicU8::new(0);
pub static KL_BANK: AtomicU8 = AtomicU8::new(0);
pub static KL_SCENE_BANK: AtomicU8 = AtomicU8::new(0);

/// Un carácter por pad: qué hay en el slot que ese pad va a lanzar.
fn clip_char(s: ClipState, has: bool) -> char {
    match s {
        ClipState::Playing => '>',
        ClipState::Triggered => '?',
        ClipState::RecQueued => 'r',
        ClipState::Recording | ClipState::Overdubbing => 'R',
        ClipState::Stopping => 'x',
        ClipState::Empty => if has { 'o' } else { '.' },
    }
}

pub struct KeylabLedHandle { stop: Arc<AtomicBool> }
impl KeylabLedHandle { pub fn stop(&self) { self.stop.store(true, Ordering::Relaxed); } }
impl Drop for KeylabLedHandle { fn drop(&mut self) { self.stop(); } }

/// ASCII imprimible y sin acentos, como `_ascii` del script .py.
/// Rellena con espacios hasta `cols` para que el LCD borre lo anterior.
fn ascii_line(s: &str, cols: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(cols);
    for ch in s.chars() {
        let c = match ch {
            'á' | 'à' | 'ä' | 'â' | 'ã' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' | 'õ' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            'Á' | 'À' | 'Ä' | 'Â' | 'Ã' => 'A',
            'É' | 'È' | 'Ë' | 'Ê' => 'E',
            'Í' | 'Ì' | 'Ï' | 'Î' => 'I',
            'Ó' | 'Ò' | 'Ö' | 'Ô' | 'Õ' => 'O',
            'Ú' | 'Ù' | 'Ü' | 'Û' => 'U',
            'Ñ' => 'N',
            '·' | '—' | '–' => '-',
            c if (' '..='~').contains(&c) => c,
            _ => ' ',
        };
        out.push(c as u8);
        if out.len() == cols { break; }
    }
    while out.len() < cols { out.push(b' '); }
    out
}

/// ARTURIA: F0 00 20 6B 7F 42 04 00 60 01 <l1> 00 02 <l2> 00 F7
fn sysex_arturia(l1: &[u8], l2: &[u8]) -> Vec<u8> {
    let mut d = vec![0xF0, 0x00, 0x20, 0x6B, 0x7F, 0x42, 0x04, 0x00, 0x60, 0x01];
    d.extend_from_slice(l1);
    d.push(0x00);
    d.push(0x02);
    d.extend_from_slice(l2);
    d.push(0x00);
    d.push(0xF7);
    d
}

/// MCU: F0 00 00 66 14 12 <off> <56 chars> F7  (off 0x00 = línea 1, 0x38 = línea 2)
fn sysex_mcu(off: u8, line: &[u8]) -> Vec<u8> {
    let mut d = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x12, off];
    let mut body = line.to_vec();
    body.resize(56, b' ');
    d.extend_from_slice(&body);
    d.push(0xF7);
    d
}

fn mode_tag(m: u8) -> &'static str {
    match m { 1 => "MIX", 2 => "SND", 3 => "DRM", _ => "DEV" }
}

fn vol_db(v: f32) -> String {
    if v <= 0.0005 { "-inf".to_string() } else { format!("{:.1}", 20.0 * v.log10()) }
}

fn pan_tag(p: f32) -> String {
    if p.abs() < 0.01 { "C".to_string() }
    else if p < 0.0 { format!("L{}", (p.abs() * 100.0).round() as i32) }
    else { format!("R{}", (p * 100.0).round() as i32) }
}

pub fn spawn(name_hint: &str, shared: Arc<SessionShared>) -> Result<KeylabLedHandle, String> {
    let out = MidiOutput::new("lanzadera-keylab-leds").map_err(|e| e.to_string())?;
    let ports = out.ports();
    let names: Vec<String> = ports.iter().filter_map(|p| out.port_name(p).ok()).collect();

    let has = |n: &str, k: &str| n.to_lowercase().contains(k);
    let idx = ports.iter().position(|p| {
            out.port_name(p).map(|n| n.contains(name_hint)).unwrap_or(false)
        })
        .or_else(|| ports.iter().position(|p| {
            out.port_name(p).map(|n| (has(&n, "keylab") || has(&n, "essential")) && has(&n, "daw")).unwrap_or(false)
        }))
        .or_else(|| ports.iter().position(|p| {
            out.port_name(p).map(|n| has(&n, "keylab") || has(&n, "essential")).unwrap_or(false)
        }));

    let Some(idx) = idx else {
        println!("[KeyLab] salidas MIDI disponibles: {:?}", names);
        return Err(format!("puerto de salida '{}' no encontrado", name_hint));
    };
    println!("[KeyLab] LCD/LEDs → {}", names.get(idx).cloned().unwrap_or_default());
    let mut conn = out.connect(&ports[idx], "lanzadera-keylab-leds").map_err(|e| e.to_string())?;

    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    std::thread::Builder::new().name("lanzadera-keylab-leds".into()).spawn(move || {
        let mut last_pads = [0xFFu8; 8];
        let mut last_btn = [0xFFu8; 4];
        let mut last1: Vec<u8> = Vec::new();
        let mut last2: Vec<u8> = Vec::new();

        // Banner inicial: confirmación visual inmediata de que el LCD responde.
        let b1 = ascii_line("lanzadera", LCD_COLS);
        let b2 = ascii_line("clip launcher", LCD_COLS);
        let _ = conn.send(&sysex_arturia(&b1, &b2));
        let _ = conn.send(&sysex_mcu(0x00, &b1));
        let _ = conn.send(&sysex_mcu(0x38, &b2));

        while !stop2.load(Ordering::Relaxed) {
            // ── LCD: banco activo + estado de los 8 pads ──
            let kl_mode = KL_MODE.load(Ordering::Relaxed);
            let kl_pads = KL_PADS_BANK.load(Ordering::Relaxed);
            let kl_bank = KL_BANK.load(Ordering::Relaxed) as usize;
            let kl_sbank = KL_SCENE_BANK.load(Ordering::Relaxed) as usize;
            let (l1, l2, playing, recording, metronome, scene) = {
                let st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                let t = st.selected_track.min(st.tracks.len().saturating_sub(1));
                let tr = st.tracks.get(t);
                let vol = tr.map(|x| x.volume).unwrap_or(0.0);
                let pan = tr.map(|x| x.pan).unwrap_or(0.0);
                let flag = if st.recording { "R" } else if st.playing { ">" } else { "-" };
                let n_tracks = st.tracks.len();
                let n_scenes = st.scenes.len().max(1);
                let (head, strip) = match kl_pads {
                    1 => {
                        // CLIPS: los 8 pads son 8 ESCENAS de la pista seleccionada,
                        // igual que el grid (pistas en filas × escenas en columnas).
                        let t = st.selected_track.min(n_tracks.saturating_sub(1));
                        let mut s = String::with_capacity(8);
                        for i in 0..8 {
                            let ss = kl_sbank + i;
                            if ss >= n_scenes { s.push(' '); continue; }
                            let has = st.tracks.get(t)
                                .and_then(|tr| tr.clips.get(ss))
                                .map(|c| c.is_some()).unwrap_or(false);
                            s.push(clip_char(shared.rt.clip_state(t, ss), has));
                        }
                        (
                            format!(
                                "CLIPS T{} S{}-{}",
                                t + 1,
                                kl_sbank + 1,
                                (kl_sbank + 8).min(n_scenes)
                            ),
                            s,
                        )
                    }
                    2 => {
                        // SCENES: '*' la elegida, 'o' con clips, '.' vacía.
                        let mut s = String::with_capacity(8);
                        for i in 0..8 {
                            let ss = kl_sbank + i;
                            if ss >= n_scenes { s.push(' '); continue; }
                            let any = st.tracks.iter().any(|x| {
                                x.clips.get(ss).map(|c| c.is_some()).unwrap_or(false)
                            });
                            s.push(if ss == st.selected_scene { '*' }
                                   else if any { 'o' } else { '.' });
                        }
                        (format!("SCENES S{}-{}", kl_sbank + 1, (kl_sbank + 8).min(n_scenes)), s)
                    }
                    _ => (
                        // En NOTES los pads disparan drums; el banco de pistas
                        // sigue afectando a faders/knobs, por eso se marca con F.
                        format!("NOTES F{}-{}", kl_bank + 1, (kl_bank + 8).min(n_tracks)),
                        format!("{} {}", vol_db(vol), pan_tag(pan)),
                    ),
                };
                (
                    head,
                    format!("{} {}{}", strip, mode_tag(kl_mode), flag),
                    st.playing, st.recording, st.metronome, st.selected_scene,
                )
            };
            let a1 = ascii_line(&l1, LCD_COLS);
            let a2 = ascii_line(&l2, LCD_COLS);
            if a1 != last1 || a2 != last2 {
                let _ = conn.send(&sysex_arturia(&a1, &a2));
                let _ = conn.send(&sysex_mcu(0x00, &a1));
                let _ = conn.send(&sysex_mcu(0x38, &a2));
                last1 = a1;
                last2 = a2;
            }

            // ── LEDs de transporte (Note On, como el script .py) ──
            for (i, (note, on)) in [
                (NOTE_PLAY, playing),
                (NOTE_STOP, !playing),
                (NOTE_REC, recording),
                (NOTE_METRO, metronome),
            ].into_iter().enumerate() {
                let v = if on { 127u8 } else { 0u8 };
                if last_btn[i] != v {
                    let _ = conn.send(&[0x90, note, v]);
                    last_btn[i] = v;
                }
            }

            // ── Pads 1-8 = pistas 1-8 en la escena cursor ──
            let exists = {
                let st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                let mut ex = [false; 8];
                for (i, e) in ex.iter_mut().enumerate() {
                    *e = st.tracks.get(i)
                        .and_then(|tr| tr.clips.get(scene))
                        .map(|c| c.is_some()).unwrap_or(false);
                }
                ex
            };
            for i in 0..8 {
                let v = match shared.rt.clip_state(i, scene) {
                    ClipState::Playing => 127,
                    ClipState::Triggered => 96,
                    ClipState::RecQueued => 80,
                    ClipState::Recording | ClipState::Overdubbing => 112,
                    ClipState::Stopping => 64,
                    ClipState::Empty => if exists[i] { 32 } else { 0 },
                };
                if v != last_pads[i] {
                    let msg = if v > 0 { [0x99, PAD_NOTES[i], v] } else { [0x89, PAD_NOTES[i], 0] };
                    let _ = conn.send(&msg);
                    last_pads[i] = v;
                }
            }

            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        // Apagado ordenado.
        for n in PAD_NOTES { let _ = conn.send(&[0x89, n, 0]); }
        for n in [NOTE_PLAY, NOTE_STOP, NOTE_REC, NOTE_METRO] { let _ = conn.send(&[0x90, n, 0]); }
        let off1 = ascii_line("", LCD_COLS);
        let off2 = ascii_line("", LCD_COLS);
        let _ = conn.send(&sysex_arturia(&off1, &off2));
        let _ = conn.send(&sysex_mcu(0x00, &off1));
        let _ = conn.send(&sysex_mcu(0x38, &off2));
    }).map_err(|e| e.to_string())?;
    Ok(KeylabLedHandle { stop })
}
