//! Arturia KeyLab Essential 49 — parsing nativo (port del script de Bitwig v7.2).
//! Dos puertos físicos: DAW (transporte/knobs/faders) y MIDI 1 (pads, canal 10).
//! Los knobs relativos mantienen una "sombra" local del valor (Fase 2: resync
//! desde SessionState para que GUI y hardware coincidan siempre).
use lanzadera_core::control::{ControlCmd, TRACK_CURSOR};
use lanzadera_core::MAX_TRACKS;

pub const KNOB_CC: [u8; 8] = [16, 17, 18, 19, 20, 21, 22, 23];
pub const CC_ENCODER: u8 = 60;
/// Knob 9: pan absoluto de la pista seleccionada (como el script).
pub const CC_KNOB9: u8 = 0;
pub const NOTE_ENC_CLICK: u8 = 84;
pub const NOTE_ENC_LEFT: u8 = 98;
pub const NOTE_ENC_RIGHT: u8 = 99;
pub const FADER9_PB_CH: u8 = 8;
pub const PAD_NOTES: [u8; 8] = [36, 37, 38, 39, 40, 41, 42, 43];
pub const PAD_CH: u8 = 9;
pub const NOTE_SAVE: u8 = 80;
pub const NOTE_UNDO: u8 = 81;
pub const NOTE_LOOP: u8 = 86;
pub const NOTE_PUNCH: u8 = 88;
pub const NOTE_METRO: u8 = 89;
pub const NOTE_RWD: u8 = 91;
pub const NOTE_FFW: u8 = 92;
pub const NOTE_STOP: u8 = 93; // SHIFT (igual que el script)
pub const NOTE_PLAY: u8 = 94;
pub const NOTE_REC: u8 = 95;

const KNOB_SENSITIVITY: f32 = 0.8;
const MAX_KNOB_STEP: i32 = 3;
/// Cercanía (0-1) para que un fader "enganche" sin saltar.
const PICKUP_WINDOW: f32 = 0.035;

use std::sync::Arc;
use lanzadera_engine::shared::SessionShared;

/// Los 4 modos del script de Bitwig (Shift+RWD cicla).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CtrlMode { Device = 0, Mixer = 1, Sends = 2, Drum = 3 }

impl CtrlMode {
    fn next(self) -> Self {
        match self {
            CtrlMode::Device => CtrlMode::Mixer,
            CtrlMode::Mixer => CtrlMode::Sends,
            CtrlMode::Sends => CtrlMode::Drum,
            CtrlMode::Drum => CtrlMode::Device,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PadsBank { Notes, Clips, Scenes }

pub struct KeyLabBridge {
    mode: CtrlMode,
    pads_bank: PadsBank,
    shift: bool,
    stop_used_as_shift: bool,
    drum_offset: u8,          // banco de celdas de Caja 0/8 (Fase 4)
    pan_shadow: [f32; 8],     // 0..1 (0.5 = centro)
    dev_shadow: [f32; 8],
    send_shadow: [f32; 8],
    /// Primera pista del banco de 8 que controlan faders/knobs (0,8,16,24).
    bank: u8,
    /// Primera escena del banco de 8 que lanzan los pads en modo SCENES.
    scene_bank: u8,
    /// Último volumen conocido por pista absoluta (para el soft-takeover).
    vol_shadow: [Option<f32>; MAX_TRACKS],
    /// Último volumen/pan conocido de la pista seleccionada (fader 9 / knob 9).
    cursor_vol: Option<f32>,
    /// Soft-takeover del fader 9 cuando actúa como master general (modo MIX).
    master_shadow: Option<f32>,
    cursor_pan: Option<f32>,
    /// Estado real del host. Sin él (tests) el pickup usa la sombra local.
    host: Option<Arc<SessionShared>>,
}

impl Default for KeyLabBridge {
    fn default() -> Self {
        KeyLabBridge {
            mode: CtrlMode::Device, // como el script
            pads_bank: PadsBank::Notes,
            shift: false,
            stop_used_as_shift: false,
            drum_offset: 0,
            pan_shadow: [0.5; 8],
            dev_shadow: [0.5; 8],
            send_shadow: [0.0; 8],
            bank: 0,
            scene_bank: 0,
            vol_shadow: [None; MAX_TRACKS],
            cursor_vol: None,
            master_shadow: None,
            cursor_pan: None,
            host: None,
        }
    }
}

impl KeyLabBridge {
    pub fn mode(&self) -> CtrlMode { self.mode }
    pub fn set_mode(&mut self, m: CtrlMode) { self.mode = m; }
    pub fn pads_bank(&self) -> PadsBank { self.pads_bank }
    pub fn drum_offset(&self) -> u8 { self.drum_offset }
    /// Primera pista del banco activo (0,8,16,24).
    pub fn bank(&self) -> u8 { self.bank }

    /// Conecta el estado real del host (lo llama connect_keylab).
    pub fn attach_host(&mut self, shared: Arc<SessionShared>) {
        self.host = Some(shared);
        self.reset_cursor_pickup();
        self.vol_shadow = [None; MAX_TRACKS];
    }

    /// Volumen real de una pista absoluta, si hay host conectado.
    fn host_volume(&self, t: usize) -> Option<f32> {
        let h = self.host.as_ref()?;
        let st = h.state.lock().unwrap_or_else(|e| e.into_inner());
        st.tracks.get(t).map(|tr| tr.volume)
    }

    /// Volumen master general real, si hay host conectado.
    fn host_master_volume(&self) -> Option<f32> {
        let h = self.host.as_ref()?;
        let st = h.state.lock().unwrap_or_else(|e| e.into_inner());
        Some(st.master_volume)
    }

    /// Volumen real de la pista seleccionada (fader 9).
    fn host_cursor_volume(&self) -> Option<f32> {
        let h = self.host.as_ref()?;
        let st = h.state.lock().unwrap_or_else(|e| e.into_inner());
        st.tracks.get(st.selected_track).map(|tr| tr.volume)
    }

    /// Pan real de la pista seleccionada, normalizado 0..1 (knob 9).
    fn host_cursor_pan(&self) -> Option<f32> {
        let h = self.host.as_ref()?;
        let st = h.state.lock().unwrap_or_else(|e| e.into_inner());
        st.tracks.get(st.selected_track).map(|tr| (tr.pan + 1.0) * 0.5)
    }

    /// Soft-takeover contra el valor REAL del host.
    ///
    /// `slot` guarda lo último que enviamos. Si el host sigue ahí, el control
    /// está enganchado y responde continuo (aunque el barrido sea rápido).
    /// Si el host se apartó (lo movieron con el ratón), se desengancha y hay
    /// que cruzar el valor real para volver a tomar el control.
    fn pickup(slot: &mut Option<f32>, value: f32, real: Option<f32>) -> bool {
        match real {
            Some(r) => {
                let engaged = slot.map(|p| (r - p).abs() <= PICKUP_WINDOW).unwrap_or(false);
                if !engaged && (value - r).abs() > PICKUP_WINDOW {
                    *slot = None;
                    return false;
                }
            }
            None => {
                if let Some(prev) = *slot {
                    if (value - prev).abs() > PICKUP_WINDOW { return false; }
                }
            }
        }
        *slot = Some(value);
        true
    }

    /// Al cambiar de pista seleccionada, fader 9 y knob 9 deben re-enganchar.
    fn reset_cursor_pickup(&mut self) {
        self.cursor_vol = None;
        self.master_shadow = None;
        self.cursor_pan = None;
    }

    /// Punto de entrada único para los dos puertos MIDI.
    pub fn feed(&mut self, status: u8, d1: u8, d2: u8, emit: &mut dyn FnMut(ControlCmd)) {
        let kind = status & 0xF0;
        let ch = status & 0x0F;
        match kind {
            0xE0 => {
                // Faders = pitch bend de 14 bits (como el script).
                let val = (((d2 as u32) << 7) | d1 as u32) as f32 / 16383.0;
                if ch == FADER9_PB_CH {
                    // En modo MIX el fader 9 es el master GENERAL (afecta a pistas y Groove; el
                    // Groove tiene su propio master en el slider del MK3).
                    if self.mode == CtrlMode::Mixer {
                        let real = self.host_master_volume();
                        if Self::pickup(&mut self.master_shadow, val, real) {
                            emit(ControlCmd::SetMasterVol { value: val });
                        }
                        return;
                    }
                    // Fader 9 = volumen de la pista seleccionada (script v7.2).
                    let real = self.host_cursor_volume();
                    if Self::pickup(&mut self.cursor_vol, val, real) {
                        emit(ControlCmd::SetVolume { track: TRACK_CURSOR, value: val });
                    }
                    return;
                }
                if ch > 7 { return; }
                match self.mode {
                    CtrlMode::Mixer => {
                        let t = (self.bank as usize + ch as usize).min(MAX_TRACKS - 1);
                        let real = self.host_volume(t);
                        if Self::pickup(&mut self.vol_shadow[t], val, real) {
                            emit(ControlCmd::SetVolume { track: t as u8, value: val });
                        }
                    }
                    CtrlMode::Device => emit(ControlCmd::DeviceParam { param: ch, value: val }),
                    CtrlMode::Sends => { if ch < 4 { emit(ControlCmd::SetSend { track: TRACK_CURSOR, send: ch, value: val }); } }
                    CtrlMode::Drum => {} // Fase 4: volumen de celdas de Caja
                }
            }
            0xB0 => self.handle_cc(d1, d2, emit),
            0x90 if d2 > 0 => self.handle_note(d1, ch, emit),
            0x80 | 0x90 => self.handle_note_off(d1, emit),
            _ => {}
        }
    }

    /// Puerto MIDI 1 ("usuario"): pads del canal 10 + teclado en vivo.
    /// Espejo de onMidiUser del script de Bitwig: aquí NUNCA llegan botones
    /// de transporte, así que cualquier nota que no sea de pad es música.
    pub fn feed_user(&mut self, status: u8, d1: u8, d2: u8, emit: &mut dyn FnMut(ControlCmd)) {
        let kind = status & 0xF0;
        let ch = status & 0x0F;
        match kind {
            0x90 if d2 > 0 => {
                if ch == PAD_CH { self.handle_pad(d1, d2, emit); }
                else { emit(ControlCmd::LiveNoteOn { track: TRACK_CURSOR, note: d1, vel: d2 }); }
            }
            0x80 | 0x90 => {
                if ch == PAD_CH {
                    if self.pads_bank == PadsBank::Notes && !self.shift {
                        if let Some(pad) = PAD_NOTES.iter().position(|&n| n == d1) {
                            emit(ControlCmd::LivePadOff { pad: (pad as u8).saturating_add(self.drum_offset) });
                        }
                    } else if self.pads_bank == PadsBank::Clips {
                        // Gate: el clip suena mientras el pad está pulsado.
                        // Al soltar se detiene (launch en press, stop en release).
                        if let Some(pad) = PAD_NOTES.iter().position(|&n| n == d1) {
                            let track = self.host_selected_track();
                            let scene = (self.scene_bank as usize + pad)
                                .min(lanzadera_core::MAX_SCENES - 1);
                            if self.host_slot_is_gate(track, scene) {
                                emit(ControlCmd::StopClip { track: track as u8 });
                                println!(
                                    "[KeyLab] Gate suelto · pista {} · escena {}",
                                    track + 1, scene + 1
                                );
                            }
                        }
                    }
                } else {
                    emit(ControlCmd::LiveNoteOff { track: TRACK_CURSOR, note: d1 });
                }
            }
            0xE0 => {
                // Pitch bend: 14 bits = (MSB<<7)|LSB, 8192 = centro.
                let value = (((d2 as u16) << 7) | d1 as u16) & 0x3FFF;
                emit(ControlCmd::LivePitchBend { track: TRACK_CURSOR, value });
            }
            0xB0 if d1 == 1 => {
                // Rueda de modulación = CC 1.
                emit(ControlCmd::LiveModWheel { track: TRACK_CURSOR, value: d2 });
            }
            0xB0 if d1 == 64 => {
                // Pedal de sustain = CC 64 (>= 64 pisado, < 64 suelto).
                emit(ControlCmd::LiveSustain { track: TRACK_CURSOR, value: d2 });
            }
            _ => {}
        }
    }

    /// Pads del canal 10. Devuelve true si el pad fue consumido.
    fn handle_pad(&mut self, note: u8, vel: u8, emit: &mut dyn FnMut(ControlCmd)) -> bool {
        let Some(pad) = PAD_NOTES.iter().position(|&n| n == note) else { return false };
        // ── Banco CLIPS: los 8 pads son 8 ESCENAS de la pista seleccionada ──
        // El grid de Lanzadera es pistas en filas × escenas en columnas, igual
        // que el cursorTrack.clipLauncherSlotBank() del script de Bitwig.
        if self.pads_bank == PadsBank::Clips {
            self.stop_used_as_shift |= self.shift;
            let scene = (self.scene_bank as usize + pad)
                .min(lanzadera_core::MAX_SCENES - 1) as u8;
            if self.shift {
                // SHIFT+pad detiene el clip que suena en la pista seleccionada.
                // Lanzar escenas completas sigue en el banco SCENES (SHIFT+METRO).
                let track = self.host_selected_track() as u8;
                emit(ControlCmd::StopClip { track });
                println!("[KeyLab] Stop clip · pista {}", track + 1);
                return true;
            }
            let track = self.host_selected_track() as u8;
            match self.host_slot_has_clip(track as usize, scene as usize) {
                Some(false) => {
                    emit(ControlCmd::CreateClip { track, scene, bars: 8 });
                    println!(
                        "[KeyLab] Clip creado · pista {} · escena {}",
                        track + 1, scene + 1
                    );
                }
                _ => {
                    emit(ControlCmd::LaunchClip { track, scene });
                    println!(
                        "[KeyLab] Lanza clip · pista {} · escena {}",
                        track + 1, scene + 1
                    );
                }
            }
            return true;
        }
        if self.shift {
            self.stop_used_as_shift = true;
            match note {
                36 => emit(ControlCmd::FillToggle),   // Shift+Pad1
                37 => self.drum_offset = if self.drum_offset == 0 { 8 } else { 0 },
                39 => emit(ControlCmd::GrooveToggle), // Shift+Pad4
                43 => emit(ControlCmd::TapTempo),     // Shift+Pad8
                _ => {}
            }
        } else {
            match self.pads_bank {
                PadsBank::Scenes => {
                    let scene = (self.scene_bank as usize + pad)
                        .min(lanzadera_core::MAX_SCENES - 1) as u8;
                    emit(ControlCmd::LaunchScene { scene });
                    println!("[KeyLab] Lanza escena {}", scene + 1);
                }
                // Banco NOTES: los pads disparan el motor de drums de Caja.
                PadsBank::Notes => emit(ControlCmd::LivePadOn {
                    pad: (pad as u8).saturating_add(self.drum_offset), vel,
                }),
                PadsBank::Clips => {} // tratado arriba
            }
        }
        true
    }
    fn handle_cc(&mut self, cc: u8, val: u8, emit: &mut dyn FnMut(ControlCmd)) {
        if cc == CC_KNOB9 {
            // Knob 9 = pan absoluto de la pista seleccionada.
            let pan = (val as f32 / 63.5 - 1.0).clamp(-1.0, 1.0);
            let real = self.host_cursor_pan();
            if Self::pickup(&mut self.cursor_pan, (pan + 1.0) * 0.5, real) {
                emit(ControlCmd::SetPan { track: TRACK_CURSOR, value: pan });
            }
            return;
        }
        if cc == CC_ENCODER {
            // Relativo del KeyLab: 1..63 = derecha (+), 65..127 = izquierda (−).
            // El script de Bitwig solo usa el SIGNO (goNext = inc > 0); aquí se
            // normaliza a ±1 por tick: un giro rápido no debe saltar 63 escenas,
            // y así toleramos ambas convenciones relativas (1/127 y 65/63).
            let raw = if val > 64 { val as i32 - 128 } else { val as i32 };
            let delta = raw.clamp(-1, 1) as i8;
            if delta == 0 { return; }
            match self.mode {
                CtrlMode::Mixer => emit(ControlCmd::SelectScene { delta }),
                _ => {
                    emit(ControlCmd::SelectTrack { delta });
                    self.reset_cursor_pickup();
                }
            }
            return;
        }
        if let Some(i) = KNOB_CC.iter().position(|&k| k == cc) {
            // Knobs relativos: 64 = centro, con sensibilidad y paso máximo del script.
            let inc = (64i32 - val as i32).clamp(-MAX_KNOB_STEP, MAX_KNOB_STEP);
            if inc == 0 { return; }
            let step = inc as f32 * KNOB_SENSITIVITY / 128.0;
            match self.mode {
                CtrlMode::Mixer => {
                    let t = (self.bank as usize + i).min(MAX_TRACKS - 1);
                    self.pan_shadow[i] = (self.pan_shadow[i] + step).clamp(0.0, 1.0);
                    emit(ControlCmd::SetPan { track: t as u8, value: self.pan_shadow[i] * 2.0 - 1.0 });
                }
                CtrlMode::Device => {
                    self.dev_shadow[i] = (self.dev_shadow[i] + step).clamp(0.0, 1.0);
                    emit(ControlCmd::DeviceParam { param: i as u8, value: self.dev_shadow[i] });
                }
                CtrlMode::Sends => {
                    if i < 4 {
                        self.send_shadow[i] = (self.send_shadow[i] + step).clamp(0.0, 1.0);
                        emit(ControlCmd::SetSend { track: TRACK_CURSOR, send: i as u8, value: self.send_shadow[i] });
                    }
                }
                CtrlMode::Drum => {} // Fase 4: pan de celdas de Caja
            }
        }
    }

    fn handle_note(&mut self, note: u8, ch: u8, emit: &mut dyn FnMut(ControlCmd)) {
        if note == NOTE_STOP {
            self.shift = true;
            self.stop_used_as_shift = false;
            return;
        }
        // Pads del canal 10 (pueden llegar por cualquiera de los dos puertos).
        if ch == PAD_CH && self.handle_pad(note, 100, emit) { return; }
        if self.shift {
            self.stop_used_as_shift = true;
            match note {
                NOTE_RWD => { self.mode = self.mode.next(); emit(ControlCmd::SetMode { mode: self.mode as u8 }); }
                NOTE_REC => emit(ControlCmd::ToggleArm { track: TRACK_CURSOR }),
                NOTE_SAVE => emit(ControlCmd::ToggleSolo { track: TRACK_CURSOR }),
                // Toggle como setPadsBank del script: repetir vuelve a NOTES.
                NOTE_METRO => self.pads_bank = if self.pads_bank == PadsBank::Scenes { PadsBank::Notes } else { PadsBank::Scenes },
                NOTE_PUNCH => self.pads_bank = if self.pads_bank == PadsBank::Clips { PadsBank::Notes } else { PadsBank::Clips },
                // SHIFT + ◄► navega la pista seleccionada (no el banco).
                NOTE_ENC_LEFT => {
                    emit(ControlCmd::SelectTrack { delta: -1 });
                    self.reset_cursor_pickup();
                }
                NOTE_ENC_RIGHT => {
                    emit(ControlCmd::SelectTrack { delta: 1 });
                    self.reset_cursor_pickup();
                }
                _ => {}
            }
            return;
        }
        match note {
            NOTE_PLAY => emit(ControlCmd::Play),
            NOTE_REC => emit(ControlCmd::Record),
            NOTE_SAVE => emit(ControlCmd::ToggleMute { track: TRACK_CURSOR }),
            NOTE_METRO => emit(ControlCmd::ToggleMetronome),
            // LOOP: para la pista seleccionada (el launcher no tiene
            // loop de arranger, así que el botón queda libre).
            NOTE_LOOP => emit(ControlCmd::StopClip { track: TRACK_CURSOR }),
            // ◄► = banco de 8 pistas en los modos de mezcla; en Device navega pista.
            NOTE_ENC_LEFT | NOTE_ENC_RIGHT => {
                let d: i16 = if note == NOTE_ENC_RIGHT { 1 } else { -1 };
                // Con los pads en CLIPS o SCENES las flechas mueven el banco
                // de ESCENAS (columnas del grid), no el de pistas.
                if matches!(self.pads_bank, PadsBank::Scenes | PadsBank::Clips) {
                    // Con los pads en SCENES, las flechas mueven ese banco.
                    let max_bank = ((lanzadera_core::MAX_SCENES - 1) / 8 * 8) as i16;
                    let next = (self.scene_bank as i16 + d * 8).clamp(0, max_bank) as u8;
                    if next != self.scene_bank {
                        self.scene_bank = next;
                        println!(
                            "[KeyLab] Banco de escenas {}-{}",
                            self.scene_bank + 1,
                            (self.scene_bank as usize + 8).min(lanzadera_core::MAX_SCENES)
                        );
                    }
                } else if matches!(self.mode, CtrlMode::Mixer | CtrlMode::Sends | CtrlMode::Drum) {
                    let max_bank = ((MAX_TRACKS - 1) / 8 * 8) as i16;
                    let next = (self.bank as i16 + d * 8).clamp(0, max_bank) as u8;
                    if next != self.bank {
                        self.bank = next;
                        println!(
                            "[KeyLab] Banco de pistas {}-{}",
                            self.bank + 1,
                            (self.bank as usize + 8).min(MAX_TRACKS)
                        );
                    }
                } else {
                    emit(ControlCmd::SelectTrack { delta: d as i8 });
                    self.reset_cursor_pickup();
                }
            }
            NOTE_RWD => emit(ControlCmd::SelectScene { delta: -1 }),
            NOTE_FFW => emit(ControlCmd::SelectScene { delta: 1 }),
            _ => {} // LOOP/PUNCH/UNDO/ENC_CLICK: pendientes de comandos (Fase A2)
        }
    }

    fn handle_note_off(&mut self, note: u8, emit: &mut dyn FnMut(ControlCmd)) {
        if note == NOTE_STOP {
            self.shift = false;
            // STOP solo también detiene el transporte (igual que el script).
            if !self.stop_used_as_shift { emit(ControlCmd::Stop); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(b: &mut KeyLabBridge, s: u8, d1: u8, d2: u8) -> Vec<ControlCmd> {
        let mut out = Vec::new();
        b.feed(s, d1, d2, &mut |c| out.push(c));
        out
    }

    #[test]
    fn fader9_es_volumen_de_la_pista_seleccionada() {
        let mut b = KeyLabBridge::default();
        let out = feed(&mut b, 0xE0 | FADER9_PB_CH, 127, 127);
        assert_eq!(out, vec![ControlCmd::SetVolume { track: TRACK_CURSOR, value: 1.0 }]);
    }

    #[test]
    fn fader9_en_mix_es_el_master_general() {
        let mut b = KeyLabBridge::default();
        b.set_mode(CtrlMode::Mixer);
        let out = feed(&mut b, 0xE0 | FADER9_PB_CH, 127, 127);
        assert_eq!(out, vec![ControlCmd::SetMasterVol { value: 1.0 }]);
    }

    #[test]
    fn fader_en_mixer_es_volumen_de_pista() {
        let mut b = KeyLabBridge::default();
        b.set_mode(CtrlMode::Mixer);
        let out = feed(&mut b, 0xE0, 127, 127);
        assert_eq!(out, vec![ControlCmd::SetVolume { track: 0, value: 1.0 }]);
    }

    #[test]
    fn shift_pad1_es_fill_y_no_dispara_stop_al_soltar() {
        let mut b = KeyLabBridge::default();
        feed(&mut b, 0x90, NOTE_STOP, 127);          // shift ON (puerto DAW)
        let out = feed(&mut b, 0x90 | PAD_CH, 36, 100); // pad1 (puerto MIDI 1)
        assert_eq!(out, vec![ControlCmd::FillToggle]);
        let out = feed(&mut b, 0x80, NOTE_STOP, 0);  // shift OFF
        assert!(out.is_empty()); // STOP se usó como shift → no detiene
    }

    #[test]
    fn stop_solo_detiene_el_transporte() {
        let mut b = KeyLabBridge::default();
        feed(&mut b, 0x90, NOTE_STOP, 127);
        assert_eq!(feed(&mut b, 0x80, NOTE_STOP, 0), vec![ControlCmd::Stop]);
    }

    #[test]
    fn shift_rwd_cicla_modo() {
        let mut b = KeyLabBridge::default(); // Device
        feed(&mut b, 0x90, NOTE_STOP, 127);
        assert_eq!(feed(&mut b, 0x90, NOTE_RWD, 127), vec![ControlCmd::SetMode { mode: 1 }]);
        assert_eq!(b.mode(), CtrlMode::Mixer);
    }


    #[test]
    fn encoder_en_mixer_selecciona_escena() {
        let mut b = KeyLabBridge::default();
        b.set_mode(CtrlMode::Mixer);
        // Convención del script: 1..63 = +, 65..127 = − (solo importa el signo).
        assert_eq!(feed(&mut b, 0xB0, CC_ENCODER, 1), vec![ControlCmd::SelectScene { delta: 1 }]);
        assert_eq!(feed(&mut b, 0xB0, CC_ENCODER, 127), vec![ControlCmd::SelectScene { delta: -1 }]);
        // Tolerancia a la otra convención relativa común (65/63): también ±1.
        assert_eq!(feed(&mut b, 0xB0, CC_ENCODER, 65), vec![ControlCmd::SelectScene { delta: -1 }]);
        assert_eq!(feed(&mut b, 0xB0, CC_ENCODER, 63), vec![ControlCmd::SelectScene { delta: 1 }]);
    }

    #[test]
    fn knob1_en_mixer_mueve_el_pan_desde_el_centro() {
        let mut b = KeyLabBridge::default();
        b.set_mode(CtrlMode::Mixer);
        let out = feed(&mut b, 0xB0, KNOB_CC[0], 63); // +1 tick
        match out[0] {
            ControlCmd::SetPan { track: 0, value } => assert!((value - 0.0125).abs() < 1e-4),
            _ => panic!("se esperaba SetPan"),
        }
    }

    fn feed_user(b: &mut KeyLabBridge, s: u8, d1: u8, d2: u8) -> Vec<ControlCmd> {
        let mut out = Vec::new();
        b.feed_user(s, d1, d2, &mut |c| out.push(c));
        out
    }

    #[test]
    fn teclado_del_puerto_usuario_suena_en_vivo() {
        let mut b = KeyLabBridge::default();
        assert_eq!(feed_user(&mut b, 0x90, 60, 100),
                   vec![ControlCmd::LiveNoteOn { track: TRACK_CURSOR, note: 60, vel: 100 }]);
        assert_eq!(feed_user(&mut b, 0x80, 60, 0),
                   vec![ControlCmd::LiveNoteOff { track: TRACK_CURSOR, note: 60 }]);
    }

    #[test]
    fn pads_en_banco_notes_disparan_notas() {
        let mut b = KeyLabBridge::default(); // banco NOTES por defecto
        assert_eq!(feed_user(&mut b, 0x90 | PAD_CH, 36, 100),
                   vec![ControlCmd::LivePadOn { pad: 0, vel: 100 }]);
    }

    #[test]
    fn botones_daw_no_generan_notas_en_vivo() {
        let mut b = KeyLabBridge::default();
        assert_eq!(feed(&mut b, 0x90, NOTE_PLAY, 127), vec![ControlCmd::Play]);
    }

    #[test]
    fn shift_pad8_es_tap_tempo() {
        let mut b = KeyLabBridge::default();
        feed(&mut b, 0x90, NOTE_STOP, 127);
        assert_eq!(feed(&mut b, 0x90 | PAD_CH, 43, 100), vec![ControlCmd::TapTempo]);
    }
}

#[cfg(test)]
mod fase_a1_tests {
    use super::*;

    fn feed(b: &mut KeyLabBridge, s: u8, d1: u8, d2: u8) -> Vec<ControlCmd> {
        let mut out = Vec::new();
        b.feed(s, d1, d2, &mut |c| out.push(c));
        out
    }

    #[test]
    fn knob9_panea_la_pista_seleccionada() {
        let mut b = KeyLabBridge::default();
        let out = feed(&mut b, 0xB0, CC_KNOB9, 127);
        match out.first() {
            Some(ControlCmd::SetPan { track, value }) => {
                assert_eq!(*track, TRACK_CURSOR);
                assert!(*value > 0.9, "127 debe ser pan a la derecha");
            }
            other => panic!("se esperaba SetPan, llegó {:?}", other),
        }
    }

    #[test]
    fn metro_alterna_metronomo() {
        let mut b = KeyLabBridge::default();
        assert_eq!(feed(&mut b, 0x90, NOTE_METRO, 127), vec![ControlCmd::ToggleMetronome]);
    }

    #[test]
    fn flechas_desplazan_el_banco_en_mixer() {
        let mut b = KeyLabBridge::default();
        b.set_mode(CtrlMode::Mixer);
        assert!(feed(&mut b, 0x90, NOTE_ENC_RIGHT, 127).is_empty());
        assert_eq!(b.bank(), 8);
        let out = feed(&mut b, 0xE0, 127, 127);
        assert_eq!(out, vec![ControlCmd::SetVolume { track: 8, value: 1.0 }]);
        feed(&mut b, 0x90, NOTE_ENC_LEFT, 127);
        assert_eq!(b.bank(), 0);
    }

    #[test]
    fn el_banco_no_se_pasa_del_maximo() {
        let mut b = KeyLabBridge::default();
        b.set_mode(CtrlMode::Mixer);
        for _ in 0..10 { feed(&mut b, 0x90, NOTE_ENC_RIGHT, 127); }
        assert_eq!(b.bank(), 24);
        for _ in 0..10 { feed(&mut b, 0x90, NOTE_ENC_LEFT, 127); }
        assert_eq!(b.bank(), 0);
    }

    #[test]
    fn el_fader_no_salta_hasta_cruzar_el_valor_conocido() {
        let mut b = KeyLabBridge::default();
        b.set_mode(CtrlMode::Mixer);
        // Primer toque: sin referencia previa, se acepta y fija la sombra.
        let out = feed(&mut b, 0xE0, 0, 64); // ~0.5
        assert_eq!(out.len(), 1);
        // Salto grande: bloqueado hasta que el fader cruce ~0.5.
        assert!(feed(&mut b, 0xE0, 127, 127).is_empty());
        // Cerca del valor conocido: engancha.
        assert_eq!(feed(&mut b, 0xE0, 4, 64).len(), 1);
    }

    #[test]
    fn shift_flechas_navega_pista_no_banco() {
        let mut b = KeyLabBridge::default();
        b.set_mode(CtrlMode::Mixer);
        feed(&mut b, 0x90, NOTE_STOP, 127);
        let out = feed(&mut b, 0x90, NOTE_ENC_RIGHT, 127);
        assert_eq!(out, vec![ControlCmd::SelectTrack { delta: 1 }]);
        assert_eq!(b.bank(), 0);
    }
}

#[cfg(test)]
mod fase_b1_tests {
    use super::*;

    fn pad(b: &mut KeyLabBridge, note: u8) -> Vec<ControlCmd> {
        let mut out = Vec::new();
        b.feed_user(0x90 | PAD_CH, note, 100, &mut |c| out.push(c));
        out
    }

    fn daw(b: &mut KeyLabBridge, status: u8, d1: u8, d2: u8) -> Vec<ControlCmd> {
        let mut out = Vec::new();
        b.feed(status, d1, d2, &mut |c| out.push(c));
        out
    }



    #[test]
    fn pads_scenes_usan_el_banco_de_escenas() {
        let mut b = KeyLabBridge::default();
        b.pads_bank = PadsBank::Scenes;
        b.scene_bank = 8;
        assert_eq!(pad(&mut b, 37), vec![ControlCmd::LaunchScene { scene: 9 }]);
    }

    #[test]
    fn flechas_mueven_el_banco_de_escenas_sin_tocar_el_de_pistas() {
        let mut b = KeyLabBridge::default();
        b.pads_bank = PadsBank::Scenes;
        assert!(daw(&mut b, 0x90, NOTE_ENC_RIGHT, 127).is_empty());
        assert_eq!(b.scene_bank, 8);
        assert_eq!(b.bank, 0);
        daw(&mut b, 0x90, NOTE_ENC_LEFT, 127);
        assert_eq!(b.scene_bank, 0);
    }

    #[test]
    fn los_atajos_globales_siguen_vivos_en_notes() {
        let mut b = KeyLabBridge::default(); // banco NOTES
        daw(&mut b, 0x90, NOTE_STOP, 127);
        assert_eq!(pad(&mut b, 36), vec![ControlCmd::FillToggle]);
        assert_eq!(pad(&mut b, 43), vec![ControlCmd::TapTempo]);
    }

}

impl KeyLabBridge {
    /// Estado para el LCD del KeyLab:
    /// (modo, banco de pads 0=NOTES 1=CLIPS 2=SCENES, banco pistas, banco escenas).
    pub fn display_state(&self) -> (u8, u8, u8, u8) {
        let pads = match self.pads_bank {
            PadsBank::Notes => 0,
            PadsBank::Clips => 1,
            PadsBank::Scenes => 2,
        };
        (self.mode as u8, pads, self.bank, self.scene_bank)
    }
}

#[cfg(test)]
mod display_tests {
    use super::*;

    #[test]
    fn el_estado_de_pantalla_refleja_modo_y_bancos() {
        let mut b = KeyLabBridge::default();
        assert_eq!(b.display_state(), (0, 0, 0, 0));
        b.set_mode(CtrlMode::Mixer);
        b.pads_bank = PadsBank::Clips;
        b.bank = 8;
        b.scene_bank = 16;
        assert_eq!(b.display_state(), (1, 1, 8, 16));
        b.pads_bank = PadsBank::Scenes;
        assert_eq!(b.display_state().1, 2);
    }
}

impl KeyLabBridge {
    /// ¿Hay clip en ese slot? `None` si no hay host conectado (tests).
    fn host_slot_has_clip(&self, track: usize, scene: usize) -> Option<bool> {
        let h = self.host.as_ref()?;
        let st = h.state.lock().unwrap_or_else(|e| e.into_inner());
        Some(
            st.tracks.get(track)
                .and_then(|tr| tr.clips.get(scene))
                .map(|c| c.is_some())
                .unwrap_or(false),
        )
    }

    /// Escena cursor real del host (0 si no hay host).
    #[allow(dead_code)] // reservado para el LCD de fases siguientes
    fn host_selected_scene(&self) -> usize {
        self.host
            .as_ref()
            .map(|h| h.state.lock().unwrap_or_else(|e| e.into_inner()).selected_scene)
            .unwrap_or(0)
    }
}

impl KeyLabBridge {
    /// Pista seleccionada real del host (0 si no hay host conectado).
    fn host_selected_track(&self) -> usize {
        self.host
            .as_ref()
            .map(|h| {
                let st = h.state.lock().unwrap_or_else(|e| e.into_inner());
                st.selected_track.min(st.tracks.len().saturating_sub(1))
            })
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod clips_orientacion_tests {
    use super::*;

    fn pad(b: &mut KeyLabBridge, note: u8) -> Vec<ControlCmd> {
        let mut out = Vec::new();
        b.feed_user(0x90 | PAD_CH, note, 100, &mut |c| out.push(c));
        out
    }

    fn daw(b: &mut KeyLabBridge, s: u8, d1: u8, d2: u8) -> Vec<ControlCmd> {
        let mut out = Vec::new();
        b.feed(s, d1, d2, &mut |c| out.push(c));
        out
    }

    #[test]
    fn los_pads_recorren_escenas_de_la_pista_seleccionada() {
        let mut b = KeyLabBridge::default();
        b.pads_bank = PadsBank::Clips;
        // Sin host la pista es 0; el pad elige la COLUMNA (escena).
        assert_eq!(pad(&mut b, 36), vec![ControlCmd::LaunchClip { track: 0, scene: 0 }]);
        assert_eq!(pad(&mut b, 43), vec![ControlCmd::LaunchClip { track: 0, scene: 7 }]);
    }

    #[test]
    fn el_banco_de_escenas_desplaza_los_pads() {
        let mut b = KeyLabBridge::default();
        b.pads_bank = PadsBank::Clips;
        b.scene_bank = 8;
        assert_eq!(pad(&mut b, 36), vec![ControlCmd::LaunchClip { track: 0, scene: 8 }]);
        assert_eq!(pad(&mut b, 43), vec![ControlCmd::LaunchClip { track: 0, scene: 15 }]);
    }

    #[test]
    fn shift_pad_detiene_el_clip_de_la_pista_seleccionada() {
        let mut b = KeyLabBridge::default();
        b.pads_bank = PadsBank::Clips;
        b.scene_bank = 8;
        daw(&mut b, 0x90, NOTE_STOP, 127);
        // Sin host conectado la pista seleccionada es 0.
        assert_eq!(pad(&mut b, 38), vec![ControlCmd::StopClip { track: 0 }]);
        // STOP se usó como shift: al soltarlo no para el transporte.
        assert!(daw(&mut b, 0x80, NOTE_STOP, 0).is_empty());
    }

    #[test]
    fn las_flechas_mueven_escenas_en_banco_clips() {
        let mut b = KeyLabBridge::default();
        b.pads_bank = PadsBank::Clips;
        assert!(daw(&mut b, 0x90, NOTE_ENC_RIGHT, 127).is_empty());
        assert_eq!(b.scene_bank, 8);
        assert_eq!(b.bank, 0, "el banco de pistas no debe moverse");
    }

    #[test]
    fn loop_detiene_la_pista_seleccionada() {
        let mut b = KeyLabBridge::default();
        assert_eq!(daw(&mut b, 0x90, NOTE_LOOP, 127),
                   vec![ControlCmd::StopClip { track: TRACK_CURSOR }]);
    }
}

impl KeyLabBridge {
    /// ¿El clip de ese slot usa LaunchMode::Gate? Sin host devuelve false:
    /// no se inventa un stop si no se conoce el modo del clip.
    fn host_slot_is_gate(&self, track: usize, scene: usize) -> bool {
        let Some(h) = self.host.as_ref() else { return false };
        let st = h.state.lock().unwrap_or_else(|e| e.into_inner());
        st.tracks.get(track)
            .and_then(|tr| tr.clips.get(scene))
            .and_then(|c| c.as_ref())
            .map(|c| c.launch_mode == lanzadera_core::LaunchMode::Gate)
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod gate_release_tests {
    use super::*;

    fn user(b: &mut KeyLabBridge, s: u8, d1: u8, d2: u8) -> Vec<ControlCmd> {
        let mut out = Vec::new();
        b.feed_user(s, d1, d2, &mut |c| out.push(c));
        out
    }

    #[test]
    fn en_notes_el_release_sigue_apagando_la_nota_del_pad() {
        let mut b = KeyLabBridge::default();
        assert_eq!(user(&mut b, 0x80 | PAD_CH, 36, 0),
                   vec![ControlCmd::LivePadOff { pad: 0 }]);
    }

    #[test]
    fn en_clips_sin_host_el_release_no_emite_stop() {
        let mut b = KeyLabBridge::default();
        b.pads_bank = PadsBank::Clips;
        assert!(user(&mut b, 0x80 | PAD_CH, 38, 0).is_empty());
    }

    #[test]
    fn el_teclado_sigue_apagando_notas_en_cualquier_banco() {
        let mut b = KeyLabBridge::default();
        b.pads_bank = PadsBank::Clips;
        assert_eq!(user(&mut b, 0x80, 60, 0),
                   vec![ControlCmd::LiveNoteOff { track: TRACK_CURSOR, note: 60 }]);
    }
}
