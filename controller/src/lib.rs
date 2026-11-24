//! lanzadera-controller — adaptadores MIDI nativos (midir).
//! Los puentes viven en el hilo de controlador: aquí SÍ se puede usar Mutex.
//! El hilo de audio jamás toca nada de este crate.
pub mod keylab;
pub mod mk3;
pub mod leds;
pub mod keylab_leds;

use std::sync::{Arc, Mutex};
use lanzadera_core::{ControlCmd, Spsc};

/// Mantiene viva la conexión midir (se cierra al hacer drop).
pub struct MidiConn(pub midir::MidiInputConnection<()>);

pub enum BridgeKind { KeyLab, Mk3 }

pub fn list_input_ports() -> Vec<String> {
    match midir::MidiInput::new("lanzadera-scan") {
        Ok(mi) => mi.ports().iter().filter_map(|p| mi.port_name(p).ok()).collect(),
        Err(_) => Vec::new(),
    }
}

fn find_port(mi: &midir::MidiInput, hint: &str) -> Option<midir::MidiInputPort> {
    mi.ports().into_iter().find(|p| mi.port_name(p).map(|n| n.contains(hint)).unwrap_or(false))
}

/// Teclado / controlador cualquiera: notas, pitch bend, rueda de modulación (CC 1) y pedal
/// de sustain (CC 64) hacia la pista seleccionada. `port_name` es el nombre del puerto de
/// entrada (o un fragmento de él).
pub fn connect_generic(port_name: &str, sink: Arc<Spsc>) -> Result<MidiConn, Box<dyn std::error::Error>> {
    use lanzadera_core::control::TRACK_CURSOR;
    let mut mi = midir::MidiInput::new("lanzadera-generic").map_err(|e| e.to_string())?;
    mi.ignore(midir::Ignore::All);
    let port = find_port(&mi, port_name)
        .ok_or_else(|| format!("puerto MIDI «{}» no encontrado", port_name))?;
    let s = sink.clone();
    let conn = mi.connect(&port, "lanzadera-generic", move |_ts, msg, _| {
        if msg.len() < 2 { return; }
        let (st, d1) = (msg[0], msg[1]);
        let d2 = *msg.get(2).unwrap_or(&0);
        let emit = |c: ControlCmd| { let _ = s.push(c.encode()); };
        match st & 0xF0 {
            0x90 if d2 > 0 => emit(ControlCmd::LiveNoteOn { track: TRACK_CURSOR, note: d1, vel: d2 }),
            0x80 | 0x90 => emit(ControlCmd::LiveNoteOff { track: TRACK_CURSOR, note: d1 }),
            0xE0 => {
                // Pitch bend: 14 bits = (MSB<<7)|LSB, 8192 = centro.
                let value = (((d2 as u16) << 7) | d1 as u16) & 0x3FFF;
                emit(ControlCmd::LivePitchBend { track: TRACK_CURSOR, value });
            }
            0xB0 if d1 == 1 => emit(ControlCmd::LiveModWheel { track: TRACK_CURSOR, value: d2 }),
            0xB0 if d1 == 64 => emit(ControlCmd::LiveSustain { track: TRACK_CURSOR, value: d2 }),
            _ => {}
        }
    }, ()).map_err(|e| e.to_string())?;
    Ok(MidiConn(conn))
}

/// Conexión genérica de un puerto (MK3 vía driver askz / snd-virmidi).
pub fn connect(kind: BridgeKind, name_hint: &str, sink: Arc<Spsc>) -> Result<MidiConn, Box<dyn std::error::Error>> {
    let mut mi = midir::MidiInput::new("lanzadera-ctrl").map_err(|e| e.to_string())?;
    mi.ignore(midir::Ignore::None);
    let port = find_port(&mi, name_hint).ok_or_else(|| format!("puerto MIDI '{}' no encontrado", name_hint))?;
    let conn = match kind {
        BridgeKind::Mk3 => {
            let mut b = mk3::Mk3Bridge::default();
            mi.connect(&port, "lanzadera-mk3", move |_ts, msg, _| {
                if msg.len() < 3 { return; }
                let (status, d1, d2) = (msg[0], msg[1], msg[2]);
                let mut emit = |cmd: ControlCmd| { let _ = sink.push(cmd.encode()); };
                match status & 0xF0 {
                    0xB0 => b.handle_cc(d1, d2, &mut emit),
                    0x90 if d2 > 0 => { b.handle_pad_note_on(d1, d2, &mut emit); }
                    0x80 | 0x90 => { b.handle_pad_note_off(d1, &mut emit); }
                    _ => {}
                }
            }, ()).map_err(|e| e.to_string())?
        }
        BridgeKind::KeyLab => {
            let mut b = keylab::KeyLabBridge::default();
            mi.connect(&port, "lanzadera-keylab", move |_ts, msg, _| {
                if msg.len() >= 3 { b.feed(msg[0], msg[1], msg[2], &mut |cmd| { let _ = sink.push(cmd.encode()); }); }
            }, ()).map_err(|e| e.to_string())?
        }
    };
    Ok(MidiConn(conn))
}

/// El KeyLab Essential 49 usa DOS puertos (como en el script de Bitwig):
/// "…DAW" (transporte/knobs/faders) y "…MIDI 1" (pads, canal 10).
/// El bridge se comparte con Arc<Mutex<>> para que el Shift del puerto DAW
/// afecte a los pads del puerto MIDI. El puerto de pads es opcional.
pub fn connect_keylab(
    sink: Arc<Spsc>,
    shared: Arc<lanzadera_engine::shared::SessionShared>,
) -> Result<Vec<MidiConn>, Box<dyn std::error::Error>> {
    let mut kb = keylab::KeyLabBridge::default();
    // El bridge necesita el estado real: el soft-takeover debe comparar contra
    // el volumen/pan del host, no contra lo último que él mismo envió.
    kb.attach_host(shared.clone());
    let bridge = Arc::new(Mutex::new(kb));
    let mut out = Vec::new();
    // ALSA TRUNCA los nombres: "…KeyLab Essential 49 MIDI 1" aparece como
    // "…KeyLab Essential 49 MID 32:0". Clasificamos por si contiene "DAW".
    for (i, want_daw) in [true, false].into_iter().enumerate() {
        let mut mi = midir::MidiInput::new("lanzadera-keylab").map_err(|e| e.to_string())?;
        mi.ignore(midir::Ignore::None);
        let found = mi.ports().into_iter().find(|p| {
            mi.port_name(p).map(|n| {
                let is_keylab = n.contains("KeyLab") || n.contains("KEYLAB") || n.contains("Keylab");
                is_keylab && (n.contains("DAW") == want_daw)
            }).unwrap_or(false)
        });
        let Some(port) = found else {
            if i == 0 { return Err("puerto DAW del KeyLab no encontrado".into()); }
            println!("[lanzadera] KeyLab: puerto de usuario (teclado/pads) NO encontrado");
            continue;
        };
        if let Ok(n) = mi.port_name(&port) {
            println!("[lanzadera] KeyLab {} → {}", if want_daw { "DAW" } else { "MIDI/usuario" }, n);
        }
        let b = bridge.clone();
        let s = sink.clone();
        let user_port = i == 1; // puerto MIDI 1 = teclado + pads (onMidiUser)
        let conn = mi.connect(&port, "lanzadera-keylab", move |_ts, msg, _| {
            if msg.len() >= 3 {
                if let Ok(mut b) = b.lock() {
                    // El pickup vive en el bridge: aquí solo se encola.
                    let emit = &mut |cmd: ControlCmd| { let _ = s.push(cmd.encode()); };
                    if user_port { b.feed_user(msg[0], msg[1], msg[2], emit); }
                    else { b.feed(msg[0], msg[1], msg[2], emit); }
                    // Estado del bridge → hilo del LCD (atomics, sin locks).
                    let (m, pb, bk, sb) = b.display_state();
                    let o = std::sync::atomic::Ordering::Relaxed;
                    crate::keylab_leds::KL_MODE.store(m, o);
                    crate::keylab_leds::KL_PADS_BANK.store(pb, o);
                    crate::keylab_leds::KL_BANK.store(bk, o);
                    crate::keylab_leds::KL_SCENE_BANK.store(sb, o);
                }
            }
        }, ()).map_err(|e| e.to_string())?;
        out.push(MidiConn(conn));
    }
    Ok(out)
}

// ════════════════════════════════════════════════════════════════════════
//  Fase 1.4C.2b — Conexión del MK3 con acceso a SessionShared
//  El bridge necesita leer `groove_machine_visible` para bifurcar su
//  comportamiento, y escribir el bitmask de botones pulsados (flash LED),
//  las peticiones de pestaña y las de ventana.
// ════════════════════════════════════════════════════════════════════════
pub fn connect_mk3(
    name_hint: &str,
    sink: Arc<Spsc>,
    shared: Arc<lanzadera_engine::shared::SessionShared>,
) -> Result<MidiConn, Box<dyn std::error::Error>> {
    use std::sync::atomic::Ordering;

    let mut mi = midir::MidiInput::new("lanzadera-mk3").map_err(|e| e.to_string())?;
    mi.ignore(midir::Ignore::None);
    let port = find_port(&mi, name_hint)
        .ok_or_else(|| format!("puerto MIDI '{}' no encontrado", name_hint))?;

    let mut b = mk3::Mk3Bridge::default();
    let mut last_gm = false;
    let mut swing_mode: u8 = 0;
    let mut pattern_mode = false;
    let mut preset_mode = false;
    // Evita procesar NoteOn repetidos del mismo pad hasta recibir NoteOff.
    let mut pressed_pads: u16 = 0;

    let conn = mi.connect(&port, "lanzadera-mk3", move |_ts, msg, _| {
        if msg.len() < 3 { return; }
        let (status, d1, d2) = (msg[0], msg[1], msg[2]);
        let kind = status & 0xF0;
        let is_cc = kind == 0xB0;
        let press = d2 > 0;

        // El MK3 puede enviar NoteOn repetidos mientras se mantiene un pad.
        // Limpiamos el bit al recibir NoteOff (0x80) o NoteOn con velocidad 0.
        if let Some(pad) = mk3::PAD_NOTES.iter().position(|&note| note == d1) {
            let bit = 1u16 << pad;
            if kind == 0x80 || (kind == 0x90 && !press) {
                pressed_pads &= !bit;
            } else if kind == 0x90 {
                if pressed_pads & bit != 0 {
                    return;
                }
                pressed_pads |= bit;
            }
        }
        let gm = shared.groove_machine_visible.load(Ordering::Relaxed);
        let ev = shared.mk3_event_mode.load(Ordering::Relaxed);
        let _sub = shared.groove_sub_mode.load(Ordering::Relaxed);
        // CHORDS (sub-modo 3): pads = dado de muestras, GROUP/AUTO/LOCK/
        // NOTE REPEAT = locks de parámetro, PITCH/MOD/PERFORM/NOTES = globales.
        let chords = gm && shared.mk3_chords_mode.load(Ordering::Relaxed);

        if gm != last_gm {
            b.set_groove_mode(gm);
            last_gm = gm;
            if !gm {
                pattern_mode = false;
                preset_mode = false;
                shared.mk3_pattern_mode.store(false, Ordering::Relaxed);
                shared.mk3_preset_mode.store(false, Ordering::Relaxed);
                swing_mode = 0;
            }
        }

        // Bitmask de botones → flash inmediato
        if is_cc && (20..=60).contains(&d1) {
            let bit = 1u64 << (d1 - 20);
            if press { shared.mk3_buttons.fetch_or(bit, Ordering::Relaxed); }
            else     { shared.mk3_buttons.fetch_and(!bit, Ordering::Relaxed); }
        }

        let flags = (b.fixed_vel as u8)
            | ((b.drum_cell_param_active as u8) << 1)
            | (if swing_mode != 0 { 1 << 2 } else { 0 });
        shared.mk3_flags.store(flags, Ordering::Relaxed);
        shared.mk3_swing_mode.store(swing_mode, Ordering::Relaxed);

        // ── SLIDER (CC 9): gain absoluto de la celda seleccionada en modo GM+Mixer ──
        if is_cc && d1 == 9 && gm {
            let sub = shared.groove_sub_mode.load(Ordering::Relaxed);
            if sub == 2 || sub == 3 {
                let pad = shared.groove_sel_pad.load(Ordering::Relaxed) as usize;
                let gain = ((d2 as u32 * 2000) / 127).min(2000) as u16;
                shared.kit.set_gain(pad, gain);
                return;
            }
        }

        // ── ENCODER ──
        if is_cc && d1 == 1 {
            let raw = if d2 >= 65 { d2 as i16 - 128 } else { d2 as i16 };
            let inc = (-raw).clamp(-8, 8) as i32;
            if inc == 0 { return; }
            if gm {
                // EVENT + pad mantenido → velocidad fina (como arrastrar con el mouse)
                let held = shared.mk3_held_pad.load(Ordering::Relaxed);
                if ev && held != 255 {
                    let fine = if b.shift_held() { 1 } else { 3 };
                    let d = (inc.signum() * fine).clamp(-20, 20) as i8;
                    shared.mk3_vel_req.store(d, Ordering::Relaxed);
                    return;
                }
                if swing_mode != 0 { shared.groove.bump(swing_mode, inc * 2); return; }
                // ── CHORDS: el encoder regula el parámetro asignado ──
                // Se edita igual con el parámetro bloqueado o libre.
                if chords {
                    let which = shared.mk3_chords_param.load(Ordering::Relaxed);
                    let pad = (shared.groove_sel_pad.load(Ordering::Relaxed) as usize).min(24);
                    if which == 255 {
                        // Sin parámetro asignado (GROUP): navegar celdas.
                        let cur = shared.groove_sel_pad.load(Ordering::Relaxed) as i32;
                        let next = (cur + inc.signum()).clamp(0, 24) as u8;
                        shared.groove_sel_pad.store(next, Ordering::Relaxed);
                        return;
                    }
                    let param = (which as usize).min(3);
                    let step = if b.shift_held() { 2 } else { 10 };
                    if let Some(v) = shared.kit.bump_param_base(pad, param, inc * step) {
                        println!(
                            "[MK3] CHORDS {} pad {} → {}‰",
                            ["", "PITCH", "DECAY", "FILTRO"][param],
                            pad + 1,
                            v
                        );
                    }
                    return;
                }
                let sub = shared.groove_sub_mode.load(Ordering::Relaxed);
                if sub == 1 {
                    let cur = shared.groove_sel_pad.load(Ordering::Relaxed) as i32;
                    let next = (cur + inc.signum()).clamp(0, 24) as u8;
                    shared.groove_sel_pad.store(next, Ordering::Relaxed);
                    return;
                }
                if sub == 2 {
                    // Modo Mixer: encoder controla PAN (el fader controla gain)
                    let pad = shared.groove_sel_pad.load(Ordering::Relaxed) as usize;
                    let cur = shared.kit.snapshot_cfg()[pad].pan;
                    // SHIFT = ajuste fino (±10), sin SHIFT = normal (±50)
                    let step = if b.shift_held() { 10 } else { 50 };
                    let next = (cur as i32 + inc * step).clamp(-1000, 1000) as i16;
                    shared.kit.set_pan(pad, next);
                    return;
                }
                let emit = |cmd: ControlCmd| { let _ = sink.push(cmd.encode()); };
                emit(ControlCmd::SelectTrack { delta: inc.signum() as i8 });
                return;
            }
        }

        // ── Peticiones de ventana / pestaña ──
        if is_cc && press {
            if gm {
                match d1 {
                    47 | 50 | 48 | 49 => {
                        let m = match d1 {
                            47 => 0,
                            50 => 1,
                            48 => 2,
                            // CHORDS alterna con la pestaña Pad.
                            _ => {
                                if shared.mk3_chords_mode.load(Ordering::Relaxed) { 0 } else { 3 }
                            }
                        };
                        shared.mk3_mode_req.store(m, Ordering::Relaxed);
                        pattern_mode = false;
                        preset_mode = false;
                        shared.mk3_pattern_mode.store(false, Ordering::Relaxed);
                        shared.mk3_preset_mode.store(false, Ordering::Relaxed);
                    }
                    52 => {
                        if b.shift_held() {
                            preset_mode = !preset_mode;
                            shared.mk3_preset_mode.store(preset_mode, Ordering::Relaxed);
                            if preset_mode {
                                pattern_mode = false;
                                shared.mk3_pattern_mode.store(false, Ordering::Relaxed);
                                shared.mk3_event_mode.store(false, Ordering::Relaxed);
                            }
                            println!(
                                "[MK3] PRESET {} — {}",
                                if preset_mode { "ON" } else { "OFF" },
                                if preset_mode {
                                    "pads 1-8 cargan; SHIFT+pad guarda"
                                } else {
                                    "pads vuelven a su modo normal"
                                }
                            );
                        } else {
                            // PATTERN normal conserva su función. Si PRESET estaba
                            // activo, el botón vuelve al modo de patrones.
                            preset_mode = false;
                            shared.mk3_preset_mode.store(false, Ordering::Relaxed);
                            pattern_mode = !pattern_mode;
                            shared.mk3_pattern_mode.store(pattern_mode, Ordering::Relaxed);
                            println!(
                                "[MK3] PATTERN {}",
                                if pattern_mode { "ON — pads = patrones" } else { "OFF" }
                            );
                        }
                    }
                    _ => {}
                }
            }
            // En CHORDS, GROUP/AUTO/LOCK/NOTE REPEAT son los locks de
            // parámetro de la celda, no toggles de ventana.
            if !chords {
                match d1 {
                    20 => shared.mk3_win_req.store(1, Ordering::Relaxed),
                    34 => shared.mk3_win_req.store(2, Ordering::Relaxed),
                    35 => shared.mk3_win_req.store(0, Ordering::Relaxed),
                    36 => shared.mk3_win_req.store(3, Ordering::Relaxed),
                    37 => shared.mk3_win_req.store(4, Ordering::Relaxed),
                    _ => {}
                }
            }
        }

        // En Groove Machine/Pad, SCENE actúa como modificador momentáneo.
        // Fuera de ese contexto el Mk3Bridge conserva LaunchScene.
        if is_cc
            && d1 == 51
            && gm
            && shared.groove_sub_mode.load(Ordering::Relaxed) == 0
        {
            return;
        }

        // ── Botones de acción de Groove Machine ──
        if is_cc && press && gm {
            match d1 {
                59 => {
                    // Clic del encoder: centrar PAN en el submodo KEYBOARD/Mixer.
                    if shared.groove_sub_mode.load(Ordering::Relaxed) == 2 {
                        let pad = (shared.groove_sel_pad.load(Ordering::Relaxed) as usize).min(24);
                        shared.kit.set_pan(pad, 0);
                        println!("[MK3] PAN centrado en el pad {}", pad + 1);
                    }
                    return;
                }
                38 => { shared.groove.reroll(true); return; }
                39 => {
                    let ph = shared.groove_phrase.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    if ph.trim().is_empty() { shared.groove.reroll(false); }
                    else { shared.groove.generate(&ph); }
                    return;
                }
                41 => { shared.groove.cycle_genre(if b.shift_held() { -1 } else { 1 }); return; }
                54 => { shared.groove.cycle_bars(if b.shift_held() { -1 } else { 1 }); return; }
                24 => { swing_mode = (swing_mode + 1) % 5;
                        shared.mk3_swing_mode.store(swing_mode, Ordering::Relaxed); return; }
                21 => {
                    match shared.groove.export_midi(false, false) {
                        Ok(p)  => println!("[MK3] MIDI exportado: {}", p.display()),
                        Err(e) => println!("[MK3] export: {}", e),
                    }
                    return;
                }
                28 => { shared.mk3_bar_req.store(if b.shift_held() { 2 } else { 0 }, Ordering::Relaxed); return; }
                29 => { shared.mk3_bar_req.store(if b.shift_held() { 2 } else { 1 }, Ordering::Relaxed); return; }
                42 if b.shift_held() => {
                    shared.mk3_bar_req.store(3, Ordering::Relaxed);
                    println!("[MK3] Ir al compás actual");
                    return;
                }
                53 => {
                    if b.shift_held() {
                        let on = !shared.mk3_follow_playhead.load(Ordering::Relaxed);
                        shared.mk3_follow_playhead.store(on, Ordering::Relaxed);
                        shared.groove_follow_playhead.store(on, Ordering::Relaxed);
                        println!("[MK3] Follow {}", if on { "ON" } else { "OFF" });
                    } else {
                        let on = !ev;
                        shared.mk3_event_mode.store(on, Ordering::Relaxed);
                        if on { shared.mk3_mode_req.store(1, Ordering::Relaxed); }
                        println!("[MK3] EVENT {}", if on { "ON — pads = 16 pasos" } else { "OFF" });
                    }
                    return;
                }
                // ── CHORDS: locks de la celda seleccionada ──
                34 | 35 | 36 | 37 if chords => {
                    let param = match d1 { 34 => 0, 35 => 1, 36 => 2, _ => 3 };
                    let pad = (shared.groove_sel_pad.load(Ordering::Relaxed) as usize).min(24);
                    let on = shared.chords_toggle_lock(pad, param);
                    // El encoder se asigna SIEMPRE (bloqueado o libre):
                    // el lock solo detiene el dado, no la edición manual.
                    let target = if param == 0 { 255 } else { param as u8 };
                    shared.mk3_chords_param.store(target, Ordering::Relaxed);
                    println!(
                        "[MK3] CHORDS {} pad {} → {}",
                        ["MUESTRA", "PITCH", "DECAY", "FILTRO"][param],
                        pad + 1,
                        if on { "LOCK" } else { "ROLL" }
                    );
                    return;
                }
                // ── CHORDS: acciones globales ──
                30 if chords => {
                    shared.chords_all_roll();
                    println!("[MK3] CHORDS: ALL ROLL");
                    return;
                }
                31 if chords => {
                    shared.chords_all_lock();
                    println!("[MK3] CHORDS: ALL LOCK (muestras fijadas)");
                    return;
                }
                32 if chords => {
                    let bars = shared.chords_toggle_rate(4);
                    println!("[MK3] CHORDS: dado cada {} compases", bars);
                    return;
                }
                33 if chords => {
                    // "Todos golpe": suenan las 25 celdas.
                    let emit = |cmd: ControlCmd| { let _ = sink.push(cmd.encode()); };
                    for p in 0..lanzadera_engine::groove_kit::NUM_CELLS as u8 {
                        emit(ControlCmd::LivePadOn { pad: p, vel: 100 });
                    }
                    println!("[MK3] CHORDS: todos golpe");
                    return;
                }
                // MUTE / SOLO de celda seleccionada (modo Groove Machine)
                58 => {
                    let pad = shared.groove_sel_pad.load(Ordering::Relaxed) as usize;
                    shared.kit.toggle_mute(pad);
                    println!("[MK3] Mute pad {}", pad + 1);
                    return;
                }
                57 => {
                    let pad = shared.groove_sel_pad.load(Ordering::Relaxed) as usize;
                    shared.kit.toggle_solo(pad);
                    println!("[MK3] Solo pad {}", pad + 1);
                    return;
                }
                _ => {}
            }
        }

        // SCENE mantenido + pad en GM/Pad: Normal -> Mute -> Solo -> Normal.
        let scene_held =
            shared.mk3_buttons.load(Ordering::Relaxed) & (1u64 << (51 - 20)) != 0;
        if kind == 0x90
            && press
            && gm
            && scene_held
            && shared.groove_sub_mode.load(Ordering::Relaxed) == 0
        {
            if let Some(pad) = mk3::PAD_NOTES.iter().position(|&n| n == d1) {
                let pad = pad.min(15);
                shared.kit.cycle_pad_scene_state(pad);
                let cfg = shared.kit.snapshot_cfg();
                let cell = &cfg[pad];
                let state = if cell.mute {
                    "MUTE"
                } else if cell.solo {
                    "SOLO"
                } else {
                    "NORMAL"
                };
                println!("[MK3] Pad {}: {}", pad + 1, state);
                return;
            }
        }

        // ── PADS ──
        if kind == 0x90 && press {
            if let Some(pad) = mk3::PAD_NOTES.iter().position(|&n| n == d1) {
                // En PRESET los ocho primeros pads son slots rápidos.
                // Esta rama precede a PATTERN/EVENT para no alterar su
                // comportamiento cuando el modo PRESET está desactivado.
                if gm && preset_mode {
                    if pad < 8 {
                        if b.shift_held() {
                            shared.request_mk3_preset_save(pad);
                        } else {
                            shared.request_mk3_preset_load(pad);
                        }
                    } else {
                        println!("[MK3] En modo PRESET se usan los pads 1–8");
                    }
                    return;
                }
                if gm && pattern_mode {
                    shared.groove.select_pattern(pad);
                    println!("[MK3] Patrón {}", pad + 1);
                    return;
                }
                if gm && ev {
                    // Ciclo de 4 estados + registrar el pad mantenido para el encoder
                    shared.mk3_held_pad.store(pad as u8, Ordering::Relaxed);
                    shared.mk3_step_req.store(pad as u8, Ordering::Relaxed);
                    return;
                }
                if chords {
                    // CHORDS: primer toque selecciona la celda; los siguientes
                    // tiran el dado y la hacen sonar. SHIFT + pad la fija.
                    let sel = shared.groove_sel_pad.load(Ordering::Relaxed) as usize;
                    if sel != pad {
                        shared.groove_sel_pad.store(pad as u8, Ordering::Relaxed);
                        println!("[MK3] CHORDS celda {}", pad + 1);
                    } else if b.shift_held() {
                        match shared.chords_pin_current(pad) {
                            Some(rel) => println!("[MK3] CHORDS fijada pad {}: {}", pad + 1, rel),
                            None => println!("[MK3] CHORDS pad {}: sin muestras", pad + 1),
                        }
                    } else {
                        shared.chords_roll(pad);
                        let emit = |cmd: ControlCmd| { let _ = sink.push(cmd.encode()); };
                        emit(ControlCmd::LivePadOn { pad: pad as u8, vel: 100 });
                    }
                    return;
                }
                if gm {
                    let sub = shared.groove_sub_mode.load(Ordering::Relaxed);
                    if sub == 1 || sub == 2 {
                        shared.groove_sel_pad.store(pad as u8, Ordering::Relaxed);
                    }
                }
            }
        }
        if kind == 0x80 || (kind == 0x90 && !press) {
            if let Some(pad) = mk3::PAD_NOTES.iter().position(|&n| n == d1) {
                if shared.mk3_held_pad.load(Ordering::Relaxed) == pad as u8 {
                    shared.mk3_held_pad.store(255, Ordering::Relaxed);
                }
            }
        }

        let mut emit = |cmd: ControlCmd| { let _ = sink.push(cmd.encode()); };
        match kind {
            0xB0 => b.handle_cc(d1, d2, &mut emit),
            0x90 if d2 > 0 => { b.handle_pad_note_on(d1, d2, &mut emit); }
            0x80 | 0x90 => { b.handle_pad_note_off(d1, &mut emit); }
            _ => {}
        }
    }, ()).map_err(|e| e.to_string())?;

    Ok(MidiConn(conn))
}
