// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! Lectura de archivos MIDI (.mid) a notas de clip (Fase G2).
//!
//! Todo ocurre en el hilo del loader, NUNCA en el callback de audio.
//! Parser: midly (Unlicense, dominio publico). API verificada en docs.rs:
//! `Smf::parse(&bytes)`, `Timing::Metrical(ticks_por_negra)`,
//! `TrackEventKind::Midi { channel, message }`.
//!
//! Lanzadera usa PPQ = 480. El fichero puede traer otra division, asi que
//! reescalamos: tick_lanzadera = tick_fichero * 480 / division.

use std::sync::atomic::Ordering;

use lanzadera_core::{Clip, MidiNote, PPQ, TICKS_PER_BAR};
use midly::{MidiMessage, Smf, Timing, TrackEventKind};

use crate::shared::SessionShared;

/// ¿Es un fichero MIDI por extension?
pub fn is_midi_file(path: &str) -> bool {
    match std::path::Path::new(path).extension().and_then(|e| e.to_str()) {
        Some(ext) => matches!(ext.to_ascii_lowercase().as_str(), "mid" | "midi"),
        None => false,
    }
}

fn push_note(out: &mut Vec<MidiNote>, ch: u8, note: u8, start: u32, dur: u32, vel: u8) {
    out.push(MidiNote {
        tick_start: start,
        tick_dur: dur.max(1),
        note,
        vel,
        channel: ch,
        occurrence: 0,   // suena en todas las repeticiones del loop
        probability: 1.0,
    });
}

/// Cierra la primera nota abierta que coincida con (canal, nota).
fn close_note(open: &mut Vec<(u8, u8, u32, u8)>, out: &mut Vec<MidiNote>,
              ch: u8, note: u8, at: u32) {
    if let Some(i) = open.iter().position(|&(c, n, _, _)| c == ch && n == note) {
        let (_, _, st, v) = open.remove(i);
        push_note(out, ch, note, st, at.saturating_sub(st), v);
    }
}

/// Parsea un .mid y devuelve (notas, compases que ocupa).
pub fn parse_midi(path: &str) -> Result<(Vec<MidiNote>, u8), String> {
    // midly presta los bytes del buffer: leer y parsear en dos pasos.
    let bytes = std::fs::read(path).map_err(|e| format!("no se pudo leer: {}", e))?;
    let smf = Smf::parse(&bytes).map_err(|e| format!("MIDI invalido: {}", e))?;

    let div: u64 = match smf.header.timing {
        Timing::Metrical(d) => (d.as_int() as u64).max(1),
        Timing::Timecode(_, _) => {
            return Err("MIDI en SMPTE (timecode) no soportado; usa MIDI metrico (ticks/negra)".into());
        }
    };

    let scale = |t: u64| -> u32 { ((t * PPQ as u64) / div).min(u32::MAX as u64) as u32 };

    let mut out: Vec<MidiNote> = Vec::new();
    let mut open: Vec<(u8, u8, u32, u8)> = Vec::new();

    // Format 0 (un track) y format 1 (varios) se fusionan en una sola lista.
    for track in &smf.tracks {
        let mut abs: u64 = 0;
        open.clear();
        for ev in track {
            abs += ev.delta.as_int() as u64;
            if let TrackEventKind::Midi { channel, message } = ev.kind {
                let ch = channel.as_int();
                match message {
                    MidiMessage::NoteOn { key, vel } => {
                        let v = vel.as_int();
                        if v == 0 {
                            // NoteOn con velocidad 0 == NoteOff (running status).
                            close_note(&mut open, &mut out, ch, key.as_int(), scale(abs));
                        } else {
                            open.push((ch, key.as_int(), scale(abs), v));
                        }
                    }
                    MidiMessage::NoteOff { key, .. } => {
                        close_note(&mut open, &mut out, ch, key.as_int(), scale(abs));
                    }
                    _ => {}
                }
            }
        }
        // Notas sin NoteOff: se cierran al final del track.
        let end = scale(abs);
        for (ch, n, st, v) in open.drain(..) {
            push_note(&mut out, ch, n, st, end.saturating_sub(st), v);
        }
    }

    if out.is_empty() {
        return Err("el MIDI no contiene notas".into());
    }
    out.sort_by_key(|n| n.tick_start);

    let max_end = out.iter()
        .map(|n| n.tick_start as u64 + n.tick_dur as u64)
        .max().unwrap_or(0);
    let bars = ((max_end + TICKS_PER_BAR - 1) / TICKS_PER_BAR).clamp(1, 255) as u8;
    Ok((out, bars))
}

/// Carga un .mid en el slot (t, s) y lo publica al hilo de audio.
/// Devuelve el numero de notas cargadas.
pub fn install_midi(shared: &SessionShared, t: usize, s: usize, path: &str) -> Result<usize, String> {
    let (notes, mut bars) = parse_midi(path)?;

    // Igual que la grabacion de audio: si Groove esta activo, la longitud del
    // clip es multiplo de su ciclo, para que el loop caiga en el 1.1 de la
    // bateria en cada vuelta.
    let g_bars: u8 = if shared.caja_sched.enabled.load(Ordering::Relaxed) {
        shared.caja_sched.schedule.lock()
            .map(|sch| ((sch.loop_ticks as u64 / TICKS_PER_BAR).max(1)).min(255) as u8)
            .unwrap_or(1)
    } else { 1 };
    if g_bars > 1 {
        let mult = ((bars as f32 / g_bars as f32).round().max(1.0)) as u8;
        bars = mult.saturating_mul(g_bars).clamp(g_bars, 255);
    }

    let name = std::path::Path::new(path)
        .file_stem().and_then(|x| x.to_str()).unwrap_or("MIDI").to_string();
    let n = notes.len();
    let q = { shared.state.lock().unwrap().quantize };

    shared.edit(|st| {
        if let Some(tr) = st.tracks.get_mut(t) {
            if let Some(slot) = tr.clips.get_mut(s) {
                let c = slot.get_or_insert_with(|| Clip {
                    name: name.clone(),
                    loop_bars: bars,
                    quantize: q,
                    ..Default::default()
                });
                c.name = name.clone();
                c.midi_notes = notes.clone();
                c.loop_bars = bars;
                // Un clip MIDI no arrastra audio de una carga anterior.
                c.audio_path = None;
            }
        }
    });

    // El slot pasa a ser MIDI: fuera el sample que hubiera.
    shared.samples.clear(t, s);
    shared.sync_midi(t, s);
    Ok(n)
}
