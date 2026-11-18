//! Runtime del Maschine Mikro MK3 para Lanzadera — port de caja/plugin/src/midi/mk3.rs.
//! CCs y PAD_NOTES idénticos a Caja; los comandos ahora son ControlCmd del launcher.
//! Fase 1: solo parsing. Los LEDs (Mk3Out) se cablean al puerto de salida en Fase 2.
use lanzadera_core::control::{ControlCmd, SCENE_CURSOR, TRACK_CURSOR};
use lanzadera_core::ClipState;

pub const CC_STAR: u8 = 21;
pub const CC_BROWSE: u8 = 22;
pub const CC_VOLUME: u8 = 23;
pub const CC_SWING: u8 = 24;
pub const CC_TEMPO: u8 = 25;
pub const CC_PLUGIN: u8 = 26;
pub const CC_SAMPLING: u8 = 27;
pub const CC_LEFT: u8 = 28;
pub const CC_RIGHT: u8 = 29;
pub const CC_GROUP: u8 = 34;
pub const CC_AUTO: u8 = 35;
pub const CC_LOCK: u8 = 36;
pub const CC_NOTE_REPEAT: u8 = 37;
pub const CC_RESTART: u8 = 38;
pub const CC_ERASE: u8 = 39;
pub const CC_TAP: u8 = 40;
pub const CC_FOLLOW: u8 = 41;
pub const CC_PLAY: u8 = 42;
pub const CC_REC: u8 = 43;
pub const CC_STOP: u8 = 44;
pub const CC_SHIFT: u8 = 45;
pub const CC_FIXED_VEL: u8 = 46;
pub const CC_PAD_MODE: u8 = 47;
pub const CC_KEYBOARD: u8 = 48;
pub const CC_CHORDS: u8 = 49;
pub const CC_STEP: u8 = 50;
pub const CC_SCENE: u8 = 51;
pub const CC_PATTERN: u8 = 52;
pub const CC_SELECT: u8 = 56;
pub const CC_SOLO: u8 = 57;
pub const CC_MUTE: u8 = 58;
pub const CC_ENCODER: u8 = 1;
pub const CC_SLIDER: u8 = 9;
pub const CC_MASCHINE: u8 = 20;
pub const CC_PITCH: u8 = 30;
pub const CC_MOD: u8 = 31;
pub const CC_PERFORM: u8 = 32;
pub const CC_NOTES_BTN: u8 = 33;
pub const CC_EVENTS: u8 = 53;
pub const CC_VARIATION: u8 = 54;
pub const CC_DUPLICATE: u8 = 55;
pub const CC_ENC_PRESS: u8 = 59;
pub const CC_ENC_TOUCH: u8 = 60;

pub const PAD_NOTES: [u8; 16] = [48, 49, 50, 51, 44, 45, 46, 47, 40, 41, 42, 43, 36, 37, 38, 39];

pub mod led {
    pub const OFF: u8 = 0;
    pub const RED: u8 = 4;
    pub const ORANGE: u8 = 11;
    pub const WARM_YELLOW: u8 = 25;
    pub const YELLOW: u8 = 32;
    pub const GREEN: u8 = 46;
    pub const CYAN: u8 = 60;
    pub const BLUE: u8 = 74;
    pub const PURPLE: u8 = 95;
    pub const WHITE: u8 = 120;
    pub const DIM: u8 = 20;
    pub const LIME: u8 = 39;
    pub const MINT: u8 = 53;
    pub const TURQUOISE: u8 = 67;
    pub const VIOLET: u8 = 88;
    pub const MAGENTA: u8 = 102;
    pub const FUCHSIA: u8 = 109;
}


/// Sub-modos del MK3 cuando Groove Machine está visible (equivalente a los modos
/// del script de Bitwig v4.8: DRUM / STEP / TRACK / CHORDS).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrooveSubMode {
    Pad,      // PAD_MODE → disparar samples (equivale a DRUM en el script)
    Step,     // STEP → editar pasos del patrón (equivale a STEP en el script)
    Mixer,    // KEYBOARD → controlar celdas gain/pan/mute (equivale a TRACK en el script)
    Browser,  // CHORDS → navegar samples (equivale a CHORDS/Samples en el script)
}

impl Default for GrooveSubMode {
    fn default() -> Self { GrooveSubMode::Pad }
}

/// Modos del MK3 en Lanzadera (spec §8.1). Caja usaba Perform por defecto;
/// aquí el launcher abre en Session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mk3Mode { Perform, Session, Scene, Mixer, StepEdit }
impl Default for Mk3Mode { fn default() -> Self { Mk3Mode::Session } }

/// Eventos de LED hacia el hardware (Fase 2: puerto MIDI de salida).
#[derive(Clone, Copy, Debug)]
pub enum Mk3Out {
    PadLed { note: u8, color: u8 },
    ButtonLed { cc: u8, value: u8 },
}

pub struct Mk3Bridge {
    groove_active: bool,
    pub groove_sub_mode: GrooveSubMode,
    pub drum_cell_param_active: bool,
    pub drum_cell_sel_pad: i8,
    pub drum_cell_param_state: [u8; 16],
    mode: Mk3Mode,
    shift: bool,
    select: bool,
    scene_bank: u8,      // 0 = escenas 1-16, 1 = escenas 17-32
    selected_scene: u8,  // escena destino del botón SCENE
    pub fixed_vel: bool,
}

impl Default for Mk3Bridge {
    fn default() -> Self {
        Mk3Bridge {
            groove_active: false,
            groove_sub_mode: GrooveSubMode::default(),
            drum_cell_param_active: false,
            drum_cell_sel_pad: -1,
            drum_cell_param_state: [0u8; 16],
            mode: Mk3Mode::default(),
            shift: false,
            select: false,
            scene_bank: 0,
            selected_scene: 0,
            fixed_vel: false,
        }
    }
}

impl Mk3Bridge {
    pub fn mode(&self) -> Mk3Mode { self.mode }
    pub fn shift_held(&self) -> bool { self.shift }
    
    /// Activa/desactiva el modo Groove Machine. Cuando está activo, los botones
    /// de modo del MK3 mapean a las pestañas de Groove Machine en vez de a los
    /// modos de Session del launcher.
    pub fn set_groove_mode(&mut self, active: bool) {
        self.groove_active = active;
        if !active {
            self.groove_sub_mode = GrooveSubMode::default();
            self.drum_cell_param_active = false;
            self.drum_cell_sel_pad = -1;
            self.drum_cell_param_state = [0u8; 16];
            self.drum_cell_param_state = [0u8; 16];
        }
    }
    pub fn scene_bank(&self) -> u8 { self.scene_bank }

    pub fn handle_cc(&mut self, cc: u8, value: u8, emit: &mut dyn FnMut(ControlCmd)) {
        let press = value > 0;
        match cc {
            CC_ENCODER => {
                // Relativo con giro invertido, igual que Caja/script v4.8.
                let raw = if value >= 65 { value as i16 - 128 } else { value as i16 };
                let inc = (-raw).clamp(-8, 8) as i8;
                if inc == 0 { return; }
                if self.groove_active {
                    // ── Groove Machine: encoder contextual al sub-modo ──
                    match self.groove_sub_mode {
                        GrooveSubMode::Pad => {
                            if self.drum_cell_param_active && self.drum_cell_sel_pad >= 0 {
                                // Ajustar Gain o Pan de la celda seleccionada
                                let pad = self.drum_cell_sel_pad as u8;
                                let state = self.drum_cell_param_state[pad as usize];
                                if state == 1 {
                                    // GAIN: emitir ajuste de volumen
                                    emit(ControlCmd::SetVolume { track: pad, value: inc as f32 / 127.0 });
                                } else if state == 2 {
                                    // PAN: emitir ajuste de pan
                                    emit(ControlCmd::SetPan { track: pad, value: inc as f32 / 64.0 });
                                }
                            } else {
                                // Sin sub-modo: cambiar banco de pads (1-8 / 9-16)
                                emit(ControlCmd::SelectTrack { delta: inc });
                            }
                        }
                        GrooveSubMode::Step => {
                            // Cambiar nota de edición (fila del step sequencer)
                            emit(ControlCmd::CajaStep { piece: 0, step: inc as u8 });
                        }
                        GrooveSubMode::Mixer => {
                            // Ajustar parámetro de la celda seleccionada
                            emit(ControlCmd::SetVolume { track: TRACK_CURSOR, value: inc as f32 / 127.0 });
                        }
                        GrooveSubMode::Browser => {
                            // Navegar carpetas/archivos
                            emit(ControlCmd::SelectTrack { delta: inc });
                        }
                    }
                } else {
                    match self.mode {
                        Mk3Mode::Scene => {
                            self.selected_scene = (self.selected_scene as i16 + inc as i16).clamp(0, 31) as u8;
                            emit(ControlCmd::SelectScene { delta: inc });
                        }
                        Mk3Mode::StepEdit => emit(ControlCmd::CajaBpm { delta: inc }),
                        _ => emit(ControlCmd::SelectTrack { delta: inc }),
                    }
                }
            }
            CC_SLIDER => emit(ControlCmd::SetMasterVol { value: value as f32 / 127.0 }),
            CC_SHIFT => self.shift = press,
            CC_SELECT => self.select = press,
            CC_PLAY if press => emit(ControlCmd::Play),
            CC_STOP if press => emit(ControlCmd::Stop),
            CC_REC if press => emit(ControlCmd::Record),
            CC_SCENE if press => emit(ControlCmd::LaunchScene { scene: self.selected_scene }),
            CC_PATTERN if press => {
                if self.groove_active { self.groove_sub_mode = GrooveSubMode::Pad; }
                else { self.mode = Mk3Mode::Session; }
            }
            CC_PAD_MODE if press => {
                if self.groove_active { self.groove_sub_mode = GrooveSubMode::Pad; }
                else { self.mode = Mk3Mode::Perform; }
            }
            CC_STEP if press => {
                if self.groove_active { self.groove_sub_mode = GrooveSubMode::Step; }
                else { self.mode = Mk3Mode::StepEdit; }
            }
            CC_KEYBOARD if press => {
                if self.groove_active { self.groove_sub_mode = GrooveSubMode::Mixer; }
                else { self.mode = Mk3Mode::Mixer; }
            }
            CC_CHORDS if press => {
                if self.groove_active { self.groove_sub_mode = GrooveSubMode::Browser; }
            }
            CC_SOLO if press => emit(ControlCmd::ToggleSolo { track: TRACK_CURSOR }),
            CC_MUTE if press => emit(ControlCmd::ToggleMute { track: TRACK_CURSOR }),
            CC_TAP if press => emit(ControlCmd::TapTempo),
            CC_VOLUME if press => {
                if self.groove_active && self.groove_sub_mode == GrooveSubMode::Pad {
                    // Sub-modo Drum Cell Param (equivale al script v4.8)
                    self.drum_cell_param_active = !self.drum_cell_param_active;
                    if !self.drum_cell_param_active {
                        self.drum_cell_sel_pad = -1;
                        self.drum_cell_param_state = [0u8; 16];
                    }
                } else {
                    emit(ControlCmd::SetMasterVol { value: 1.0 }); // fallback
                }
            }
            CC_FIXED_VEL if press => self.fixed_vel = !self.fixed_vel,
            CC_LEFT if press => { self.scene_bank = self.scene_bank.saturating_sub(1); emit(ControlCmd::SelectScene { delta: -1 }); }
            CC_RIGHT if press => { self.scene_bank = (self.scene_bank + 1).min(1); emit(ControlCmd::SelectScene { delta: 1 }); }
            _ => {}
        }
    }

    /// Devuelve true si el pad fue consumido por el bridge.
    pub fn handle_pad_note_on(&mut self, note: u8, vel: u8, emit: &mut dyn FnMut(ControlCmd)) -> bool {
        let Some(pad) = PAD_NOTES.iter().position(|&n| n == note) else { return false };
        match self.mode {
            Mk3Mode::Session => {
                if self.groove_active {
                    // ── Groove Machine: pads según sub-modo ──
                    match self.groove_sub_mode {
                        GrooveSubMode::Pad => {
                            if self.drum_cell_param_active {
                                // Sub-modo Drum Cell Param: ciclar Gain→Pan→Mute
                                self.handle_drum_cell_param_pad(pad, emit);
                            } else {
                                emit(ControlCmd::LivePadOn { pad: pad as u8, vel });
                            }
                        }
                        GrooveSubMode::Step => {
                            // Editar paso del patrón activo
                            emit(ControlCmd::CajaStep { piece: pad as u8, step: 0 });
                        }
                        GrooveSubMode::Mixer => {
                            // Seleccionar celda del mixer
                            emit(ControlCmd::SelectClip { track: pad as u8, scene: SCENE_CURSOR });
                        }
                        GrooveSubMode::Browser => {
                            // Navegar carpetas (encoder hace el trabajo pesado)
                            emit(ControlCmd::SelectTrack { delta: if pad < 8 { -1 } else { 1 } });
                        }
                    }
                } else {
                    let scene = (self.scene_bank * 16 + pad as u8).min(31);
                    if self.select { emit(ControlCmd::SelectClip { track: TRACK_CURSOR, scene }); }
                    else if self.shift { emit(ControlCmd::StopClip { track: TRACK_CURSOR }); }
                    else { emit(ControlCmd::LaunchClip { track: TRACK_CURSOR, scene }); }
                }
                true
            }
            Mk3Mode::Scene => {
                let scene = (self.scene_bank * 16 + pad as u8).min(31);
                self.selected_scene = scene;
                emit(ControlCmd::LaunchScene { scene });
                true
            }
            Mk3Mode::Mixer => {
                // Pad = seleccionar pista (SelectClip también mueve el cursor de pista).
                emit(ControlCmd::SelectClip { track: pad as u8, scene: SCENE_CURSOR });
                true
            }
            Mk3Mode::StepEdit => {
                // Fase 4: el paso real lo fija el editor de Caja.
                emit(ControlCmd::CajaStep { piece: pad as u8, step: 0 });
                true
            }
            Mk3Mode::Perform => {
                // Fase 4A: en Perform los pads disparan el motor de Caja.
                emit(ControlCmd::LivePadOn { pad: pad as u8, vel });
                true
            }
        }
    }

    pub fn handle_pad_note_off(&mut self, note: u8, emit: &mut dyn FnMut(ControlCmd)) -> bool {
        if self.mode == Mk3Mode::Perform {
            if let Some(pad) = PAD_NOTES.iter().position(|&n| n == note) {
                emit(ControlCmd::LivePadOff { pad: pad as u8 });
                return true;
            }
        }
        PAD_NOTES.iter().any(|&n| n == note)
    }

    /// Colores de los 16 pads en modo Session (Fase 2 los envía por MIDI out).
    pub fn session_pad_colors(&self, states: &[ClipState; 16]) -> [u8; 16] {
        let mut out = [led::OFF; 16];
        for i in 0..16 {
            out[i] = match states[i] {
                ClipState::Playing => led::GREEN,
                ClipState::Triggered => led::YELLOW,
                ClipState::RecQueued => led::MAGENTA,
                ClipState::Recording | ClipState::Overdubbing => led::RED,
                ClipState::Stopping => led::ORANGE,
                ClipState::Empty => led::DIM,
            };
        }
        out
    }


    /// Colores de los 16 pads cuando Groove Machine está activo.
    /// Equivale a updateNotePadLeds / updateStepLeds / updateTrackLeds del script v4.8.
    pub fn groove_pad_colors(&self, groove_step: u8, playing: bool) -> [u8; 16] {
        let mut out = [led::OFF; 16];
        match self.groove_sub_mode {
            GrooveSubMode::Pad => {
                if self.drum_cell_param_active {
                    // Sub-modo Drum Cell Param: LED según estado de cada pad
                    for i in 0..16 {
                        let state = self.drum_cell_param_state[i];
                        let is_sel = self.drum_cell_sel_pad == i as i8;
                        out[i] = match state {
                            1 => if is_sel { led::CYAN } else { led::DIM },   // GAIN
                            2 => if is_sel { led::LIME } else { led::DIM },   // PAN
                            3 => led::ORANGE,                                  // MUTE ON
                            _ => led::OFF,
                        };
                    }
                } else {
                    // Modo Pad normal: pads apagados (se iluminan al disparar)
                    out.fill(led::DIM);
                }
            }
            GrooveSubMode::Step => {
                // Playhead blanco en el paso actual, pasos activos en blanco,
                // celdas vacías en amarillo pálido (STEP_IDLE_COLOR = 22)
                let step_idle: u8 = 22;
                for i in 0..16 {
                    if playing && i as u8 == groove_step {
                        out[i] = led::WHITE;  // playhead
                    } else {
                        out[i] = step_idle;   // idle (se actualizará con datos del schedule)
                    }
                }
            }
            GrooveSubMode::Mixer => {
                // Fila 0: selección (blanco/cyan) · Fila 1: mute (naranja) ·
                // Fila 2: solo (amarillo) · Fila 3: arm (rojo)
                for i in 0..16 {
                    out[i] = match i / 4 {
                        0 => led::CYAN,     // selección
                        1 => led::ORANGE,   // mute
                        2 => led::YELLOW,   // solo
                        _ => led::RED,      // arm
                    };
                }
            }
            GrooveSubMode::Browser => {
                // Todos en turquesa (navegación de carpetas)
                out.fill(led::TURQUOISE);
            }
        }
        out
    }

    /// Colores de los 16 pads para el motor de Caja (Fase 4A).
    /// Ilumina el paso actual en verde brillante, el resto apagado.
    pub fn caja_pad_colors(&self, caja_step: u8) -> [u8; 16] {
        let mut out = [led::OFF; 16];
        if caja_step != 255 && caja_step < 16 {
            out[caja_step as usize] = led::GREEN;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cc(b: &mut Mk3Bridge, c: u8, v: u8) -> Vec<ControlCmd> {
        let mut out = Vec::new();
        b.handle_cc(c, v, &mut |cmd| out.push(cmd));
        out
    }
    fn pad(b: &mut Mk3Bridge, note: u8) -> Vec<ControlCmd> {
        let mut out = Vec::new();
        b.handle_pad_note_on(note, 100, &mut |cmd| out.push(cmd));
        out
    }

    #[test]
    fn slider_es_master() {
        let mut b = Mk3Bridge::default();
        let out = cc(&mut b, CC_SLIDER, 127);
        assert!(matches!(out[0], ControlCmd::SetMasterVol { value } if value > 0.99));
    }

    #[test]
    fn encoder_invertido_selecciona_pista() {
        let mut b = Mk3Bridge::default();
        assert_eq!(cc(&mut b, CC_ENCODER, 1), vec![ControlCmd::SelectTrack { delta: -1 }]);
        assert_eq!(cc(&mut b, CC_ENCODER, 127), vec![ControlCmd::SelectTrack { delta: 1 }]);
    }

    #[test]
    fn pads_session_lanzan_clips_del_cursor() {
        let mut b = Mk3Bridge::default();
        assert_eq!(pad(&mut b, 48), vec![ControlCmd::LaunchClip { track: TRACK_CURSOR, scene: 0 }]);
        assert_eq!(pad(&mut b, 39), vec![ControlCmd::LaunchClip { track: TRACK_CURSOR, scene: 15 }]);
        cc(&mut b, CC_SHIFT, 127);
        assert_eq!(pad(&mut b, 48), vec![ControlCmd::StopClip { track: TRACK_CURSOR }]);
    }

    #[test]
    fn boton_scene_lanza_la_escena_seleccionada() {
        let mut b = Mk3Bridge::default();
        assert_eq!(cc(&mut b, CC_SCENE, 127), vec![ControlCmd::LaunchScene { scene: 0 }]);
    }

    #[test]
    fn banco_de_escenas_con_left_right() {
        let mut b = Mk3Bridge::default();
        cc(&mut b, CC_RIGHT, 127);
        assert_eq!(pad(&mut b, 48), vec![ControlCmd::LaunchClip { track: TRACK_CURSOR, scene: 16 }]);
    }
}

// ── Sub-modo Drum Cell Param (lógica exacta del script Bitwig v4.8) ──
impl Mk3Bridge {
    /// Cicla Gain→Pan→Mute de la celda que el pad dispara en modo Pad.
    /// Ciclo por pad: 0=OFF  1=GAIN (encoder=volumen)  2=PAN (encoder=paneo)
    ///                3=MUTE_ON → al presionar de nuevo: desmutar + volver a 0
    /// El LED del ciclo queda en el pad físico presionado (no en el display remapeado).
    fn handle_drum_cell_param_pad(&mut self, pad: usize, emit: &mut dyn FnMut(ControlCmd)) {
        let state = self.drum_cell_param_state[pad];
        match state {
            0 => {
                // OFF → GAIN: iluminar cian, asignar este pad al encoder
                self.drum_cell_param_state[pad] = 1;
                self.drum_cell_sel_pad = pad as i8;
            }
            1 => {
                // GAIN → PAN: cambiar a verde
                self.drum_cell_param_state[pad] = 2;
                self.drum_cell_sel_pad = pad as i8;
            }
            2 => {
                // PAN → MUTE ON: mutar celda, naranja; encoder ya no controla este pad
                self.drum_cell_param_state[pad] = 3;
                self.drum_cell_sel_pad = -1;
                emit(ControlCmd::ToggleMute { track: pad as u8 });
            }
            _ => {
                // MUTE ON → OFF: desmutar, apagar LED
                self.drum_cell_param_state[pad] = 0;
                if self.drum_cell_sel_pad == pad as i8 { self.drum_cell_sel_pad = -1; }
                emit(ControlCmd::ToggleMute { track: pad as u8 });
            }
        }
    }
}
