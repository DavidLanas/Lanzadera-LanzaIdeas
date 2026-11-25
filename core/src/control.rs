//! ControlCmd — comando unificado de cualquier controlador, empaquetado en u32
//! para viajar por el Spsc (mismo patrón que Mk3Cmd → u32 de Caja).
//!
//! Layout del u32:
//!   [31..26] opcode (6 bits)
//!   [25..21] a      (5 bits: track / param / modo / slot / piece)
//!   [20..16] b      (5 bits: scene / send)
//!   [15..2]  val14  (14 bits: valor 0..16383, o bpm×10, o delta+8192, o step)
//!   [1]      flag SCENE_CURSOR
//!   [0]      flag TRACK_CURSOR
//!
//! Los faders del KeyLab son pitch-bend de 14 bits → precisión exacta.
//! Los valores continuos se normalizan a 0..1 (pan se mapea de -1..+1).

/// Sentinela: "la pista/escena seleccionada ahora mismo" (se resuelve en la UI).
pub const TRACK_CURSOR: u8 = 0xFF;
pub const SCENE_CURSOR: u8 = 0xFF;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ControlCmd {
    // Clips
    LaunchClip { track: u8, scene: u8 },
    StopClip { track: u8 },
    LaunchScene { scene: u8 },
    StopAll,
    ToggleMetronome,
    // Transporte
    Play,
    Stop,
    Record,
    TapTempo,
    SetTempo { bpm: f32 },
    // Mixer
    SetVolume { track: u8, value: f32 },
    SetPan { track: u8, value: f32 },
    SetMasterVol { value: f32 },
    /// Master SOLO del Groove Machine (el master general afecta a todo).
    SetGrooveMasterVol { value: f32 },
    ToggleMute { track: u8 },
    ToggleSolo { track: u8 },
    ToggleArm { track: u8 },
    // Navegación
    SelectTrack { delta: i8 },
    SelectScene { delta: i8 },
    SelectClip { track: u8, scene: u8 },
    // Dispositivo
    DeviceParam { param: u8, value: f32 },
    // Modos de controlador
    SetMode { mode: u8 },
    // Caja (Fase 4)
    CajaPattern { slot: u8 },
    CajaStep { piece: u8, step: u8 },
    CajaBpm { delta: i8 },
    // Fill / Groove
    FillToggle,
    /// Extensión requerida por el mapeo KeyLab (Shift+Pad4 del spec §8.2).
    GrooveToggle,
    /// Extensión requerida por el modo SENDS del KeyLab (spec §8.2).
    SetSend { track: u8, send: u8, value: f32 },
    /// Nota en vivo del teclado (Fase 3C): va directa al audio, sin pasar
    /// por el estado. note y vel viajan juntos en val14 (7+7 bits).
    LiveNoteOn { track: u8, note: u8, vel: u8 },
    LiveNoteOff { track: u8, note: u8 },
    /// Pitch bend (14 bits, 8192 = centro) y rueda de modulación (CC1).
    LivePitchBend { track: u8, value: u16 },
    LiveModWheel { track: u8, value: u8 },
    /// Pedal de sustain (CC 64): value >= 64 = pisado, < 64 = suelto.
    LiveSustain { track: u8, value: u8 },
    /// Fase 3G: setear valor de parámetro del plugin (0-1 normalizado).
    SetPluginParam { track: u8, param_idx: u8, value: f32 },
    /// Seleccionar parámetro activo del plugin (para el encoder).
    SelectPluginParam { track: u8, param_idx: u8 },
    /// Fase 4A: pads físicos → motor de drums de Caja (global, sin pista).
    LivePadOn { pad: u8, vel: u8 },
    LivePadOff { pad: u8 },
    /// Crea un clip vacío en el slot si está libre (no pisa contenido).
    /// `bars` = longitud del loop en compases.
    CreateClip { track: u8, scene: u8, bars: u8 },
}

const OP_LAUNCH_CLIP: u32 = 1;
const OP_STOP_CLIP: u32 = 2;
const OP_LAUNCH_SCENE: u32 = 3;
const OP_STOP_ALL: u32 = 4;
const OP_PLAY: u32 = 5;
const OP_STOP: u32 = 6;
const OP_RECORD: u32 = 7;
const OP_TAP_TEMPO: u32 = 8;
const OP_SET_TEMPO: u32 = 9;
const OP_SET_VOLUME: u32 = 10;
const OP_SET_PAN: u32 = 11;
const OP_SET_MASTER: u32 = 12;
const OP_MUTE: u32 = 13;
const OP_SOLO: u32 = 14;
const OP_ARM: u32 = 15;
const OP_SEL_TRACK: u32 = 16;
const OP_SEL_SCENE: u32 = 17;
const OP_SEL_CLIP: u32 = 18;
const OP_DEV_PARAM: u32 = 19;
const OP_SET_MODE: u32 = 20;
const OP_CAJA_PAT: u32 = 21;
const OP_CAJA_STEP: u32 = 22;
const OP_CAJA_BPM: u32 = 23;
const OP_FILL: u32 = 24;
const OP_GROOVE: u32 = 25;
const OP_SET_SEND: u32 = 26;
const OP_LIVE_ON: u32 = 27;
const OP_LIVE_OFF: u32 = 28;
const OP_LIVE_BEND: u32 = 29;
const OP_LIVE_MOD: u32 = 30;
const OP_SET_PARAM: u32 = 31;
const OP_SEL_PARAM: u32 = 32;
const OP_LIVE_PAD_ON: u32 = 33;
const OP_LIVE_PAD_OFF: u32 = 34;
const OP_TOGGLE_METRONOME: u32 = 35;
const OP_CREATE_CLIP: u32 = 36;
const OP_LIVE_SUSTAIN: u32 = 37;
const OP_SET_GROOVE_MASTER: u32 = 38;

#[inline] fn pack_unit(v: f32) -> u32 { (v.clamp(0.0, 1.0) * 16383.0).round() as u32 }
#[inline] fn unpack_unit(v: u32) -> f32 { v as f32 / 16383.0 }
#[inline] fn pack_delta(d: i8) -> u32 { (d as i32 + 8192) as u32 }
#[inline] fn unpack_delta(v: u32) -> i8 { (v as i32 - 8192).clamp(-128, 127) as i8 }

impl ControlCmd {
    pub fn encode(self) -> u32 {
        // (op, a, b, val14, flag_track_cursor, flag_scene_cursor)
        let (op, a, b, v, tc, sc) = match self {
            ControlCmd::LaunchClip { track, scene } => (OP_LAUNCH_CLIP, track, scene, 0, track == TRACK_CURSOR, scene == SCENE_CURSOR),
            ControlCmd::StopClip { track } => (OP_STOP_CLIP, track, 0, 0, track == TRACK_CURSOR, false),
            ControlCmd::LaunchScene { scene } => (OP_LAUNCH_SCENE, 0, scene, 0, false, scene == SCENE_CURSOR),
            ControlCmd::StopAll => (OP_STOP_ALL, 0, 0, 0, false, false),
            ControlCmd::Play => (OP_PLAY, 0, 0, 0, false, false),
            ControlCmd::Stop => (OP_STOP, 0, 0, 0, false, false),
            ControlCmd::Record => (OP_RECORD, 0, 0, 0, false, false),
            ControlCmd::TapTempo => (OP_TAP_TEMPO, 0, 0, 0, false, false),
            ControlCmd::SetTempo { bpm } => (OP_SET_TEMPO, 0, 0, (bpm.clamp(0.0, 1638.3) * 10.0).round() as u32, false, false),
            ControlCmd::SetVolume { track, value } => (OP_SET_VOLUME, track, 0, pack_unit(value), track == TRACK_CURSOR, false),
            ControlCmd::SetPan { track, value } => (OP_SET_PAN, track, 0, pack_unit((value.clamp(-1.0, 1.0) + 1.0) * 0.5), track == TRACK_CURSOR, false),
            ControlCmd::SetMasterVol { value } => (OP_SET_MASTER, 0, 0, pack_unit(value), false, false),
            ControlCmd::SetGrooveMasterVol { value } => (OP_SET_GROOVE_MASTER, 0, 0, pack_unit(value), false, false),
            ControlCmd::ToggleMute { track } => (OP_MUTE, track, 0, 0, track == TRACK_CURSOR, false),
            ControlCmd::ToggleSolo { track } => (OP_SOLO, track, 0, 0, track == TRACK_CURSOR, false),
            ControlCmd::ToggleArm { track } => (OP_ARM, track, 0, 0, track == TRACK_CURSOR, false),
            ControlCmd::SelectTrack { delta } => (OP_SEL_TRACK, 0, 0, pack_delta(delta), false, false),
            ControlCmd::SelectScene { delta } => (OP_SEL_SCENE, 0, 0, pack_delta(delta), false, false),
            ControlCmd::SelectClip { track, scene } => (OP_SEL_CLIP, track, scene, 0, track == TRACK_CURSOR, scene == SCENE_CURSOR),
            ControlCmd::DeviceParam { param, value } => (OP_DEV_PARAM, param, 0, pack_unit(value), false, false),
            ControlCmd::SetMode { mode } => (OP_SET_MODE, mode, 0, 0, false, false),
            ControlCmd::CajaPattern { slot } => (OP_CAJA_PAT, slot, 0, 0, false, false),
            ControlCmd::CajaStep { piece, step } => (OP_CAJA_STEP, piece, 0, step as u32, false, false),
            ControlCmd::CajaBpm { delta } => (OP_CAJA_BPM, 0, 0, pack_delta(delta), false, false),
            ControlCmd::FillToggle => (OP_FILL, 0, 0, 0, false, false),
            ControlCmd::GrooveToggle => (OP_GROOVE, 0, 0, 0, false, false),
            ControlCmd::SetSend { track, send, value } => (OP_SET_SEND, track, send, pack_unit(value), track == TRACK_CURSOR, false),
            ControlCmd::LiveNoteOn { track, note, vel } => (OP_LIVE_ON, track, 0,
                (note as u32 & 0x7F) | ((vel as u32 & 0x7F) << 7), track == TRACK_CURSOR, false),
            ControlCmd::LiveNoteOff { track, note } => (OP_LIVE_OFF, track, 0, note as u32 & 0x7F, track == TRACK_CURSOR, false),
            ControlCmd::LivePitchBend { track, value } => (OP_LIVE_BEND, track, 0, value as u32 & 0x3FFF, track == TRACK_CURSOR, false),
            ControlCmd::LiveModWheel { track, value } => (OP_LIVE_MOD, track, 0, value as u32 & 0x7F, track == TRACK_CURSOR, false),
            ControlCmd::LiveSustain { track, value } => (OP_LIVE_SUSTAIN, track, 0, value as u32 & 0x7F, track == TRACK_CURSOR, false),
            ControlCmd::SetPluginParam { track, param_idx, value } => (OP_SET_PARAM, track, param_idx, pack_unit(value), track == TRACK_CURSOR, false),
            ControlCmd::SelectPluginParam { track, param_idx } => (OP_SEL_PARAM, track, param_idx, 0, track == TRACK_CURSOR, false),
            ControlCmd::LivePadOn { pad, vel } => (OP_LIVE_PAD_ON, pad, 0, vel as u32 & 0x7F, false, false),
            ControlCmd::LivePadOff { pad } => (OP_LIVE_PAD_OFF, pad, 0, 0, false, false),
            ControlCmd::ToggleMetronome => (OP_TOGGLE_METRONOME, 0, 0, 0, false, false),
            ControlCmd::CreateClip { track, scene, bars } => (OP_CREATE_CLIP, track, scene,
                bars as u32, track == TRACK_CURSOR, scene == SCENE_CURSOR),
        };
        let a5 = if tc { 0 } else { a.min(31) } as u32;
        let b5 = if sc { 0 } else { b.min(31) } as u32;
        (op << 26) | (a5 << 21) | (b5 << 16) | ((v & 0x3FFF) << 2) | ((sc as u32) << 1) | (tc as u32)
    }

    pub fn decode(raw: u32) -> Option<ControlCmd> {
        let op = (raw >> 26) & 0x3F;
        let a = ((raw >> 21) & 0x1F) as u8;
        let b = ((raw >> 16) & 0x1F) as u8;
        let v = (raw >> 2) & 0x3FFF;
        let tc = raw & 1 != 0;
        let sc = (raw >> 1) & 1 != 0;
        let track = if tc { TRACK_CURSOR } else { a };
        let scene = if sc { SCENE_CURSOR } else { b };
        Some(match op {
            OP_LAUNCH_CLIP => ControlCmd::LaunchClip { track, scene },
            OP_STOP_CLIP => ControlCmd::StopClip { track },
            OP_LAUNCH_SCENE => ControlCmd::LaunchScene { scene },
            OP_STOP_ALL => ControlCmd::StopAll,
            OP_TOGGLE_METRONOME => ControlCmd::ToggleMetronome,
            OP_PLAY => ControlCmd::Play,
            OP_STOP => ControlCmd::Stop,
            OP_RECORD => ControlCmd::Record,
            OP_TAP_TEMPO => ControlCmd::TapTempo,
            OP_SET_TEMPO => ControlCmd::SetTempo { bpm: v as f32 / 10.0 },
            OP_SET_VOLUME => ControlCmd::SetVolume { track, value: unpack_unit(v) },
            OP_SET_PAN => ControlCmd::SetPan { track, value: unpack_unit(v) * 2.0 - 1.0 },
            OP_SET_MASTER => ControlCmd::SetMasterVol { value: unpack_unit(v) },
            OP_SET_GROOVE_MASTER => ControlCmd::SetGrooveMasterVol { value: unpack_unit(v) },
            OP_MUTE => ControlCmd::ToggleMute { track },
            OP_SOLO => ControlCmd::ToggleSolo { track },
            OP_ARM => ControlCmd::ToggleArm { track },
            OP_SEL_TRACK => ControlCmd::SelectTrack { delta: unpack_delta(v) },
            OP_SEL_SCENE => ControlCmd::SelectScene { delta: unpack_delta(v) },
            OP_SEL_CLIP => ControlCmd::SelectClip { track, scene },
            OP_DEV_PARAM => ControlCmd::DeviceParam { param: a, value: unpack_unit(v) },
            OP_SET_MODE => ControlCmd::SetMode { mode: a },
            OP_CAJA_PAT => ControlCmd::CajaPattern { slot: a },
            OP_CAJA_STEP => ControlCmd::CajaStep { piece: a, step: v.min(255) as u8 },
            OP_CAJA_BPM => ControlCmd::CajaBpm { delta: unpack_delta(v) },
            OP_FILL => ControlCmd::FillToggle,
            OP_GROOVE => ControlCmd::GrooveToggle,
            OP_SET_SEND => ControlCmd::SetSend { track, send: b, value: unpack_unit(v) },
            OP_LIVE_ON => ControlCmd::LiveNoteOn { track, note: (v & 0x7F) as u8, vel: ((v >> 7) & 0x7F) as u8 },
            OP_LIVE_OFF => ControlCmd::LiveNoteOff { track, note: (v & 0x7F) as u8 },
            OP_LIVE_BEND => ControlCmd::LivePitchBend { track, value: (v & 0x3FFF) as u16 },
            OP_LIVE_MOD => ControlCmd::LiveModWheel { track, value: (v & 0x7F) as u8 },
            OP_LIVE_SUSTAIN => ControlCmd::LiveSustain { track, value: (v & 0x7F) as u8 },
            OP_SET_PARAM => ControlCmd::SetPluginParam { track, param_idx: b, value: unpack_unit(v) },
            OP_SEL_PARAM => ControlCmd::SelectPluginParam { track, param_idx: b },
            OP_LIVE_PAD_ON => ControlCmd::LivePadOn { pad: a, vel: (v & 0x7F) as u8 },
            OP_LIVE_PAD_OFF => ControlCmd::LivePadOff { pad: a },
            OP_CREATE_CLIP => ControlCmd::CreateClip { track, scene, bars: v.min(255) as u8 },
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt(c: ControlCmd) -> ControlCmd { ControlCmd::decode(c.encode()).unwrap() }

    #[test]
    fn roundtrip_discretos() {
        for c in [
            ControlCmd::LaunchClip { track: 3, scene: 7 },
            ControlCmd::StopClip { track: 31 },
            ControlCmd::LaunchScene { scene: 12 },
            ControlCmd::SelectClip { track: 1, scene: 2 },
            ControlCmd::SetMode { mode: 3 },
            ControlCmd::CajaPattern { slot: 9 },
            ControlCmd::CajaStep { piece: 5, step: 63 },
            ControlCmd::StopAll,
    ControlCmd::ToggleMetronome, ControlCmd::Play, ControlCmd::Stop,
            ControlCmd::Record, ControlCmd::TapTempo, ControlCmd::FillToggle, ControlCmd::GrooveToggle,
        ] { assert_eq!(rt(c), c); }
    }

    #[test]
    fn roundtrip_sentinelas() {
        assert_eq!(rt(ControlCmd::LaunchClip { track: TRACK_CURSOR, scene: SCENE_CURSOR }),
                   ControlCmd::LaunchClip { track: TRACK_CURSOR, scene: SCENE_CURSOR });
        // El valor viaja cuantizado a 14 bits (16383 es impar: 0.5 no es
        // exacto, error ~3e-5). Igual que el pitch-bend real del KeyLab.
        match rt(ControlCmd::SetVolume { track: TRACK_CURSOR, value: 0.5 }) {
            ControlCmd::SetVolume { track, value } => {
                assert_eq!(track, TRACK_CURSOR);
                assert!((value - 0.5).abs() < 1e-4);
            }
            _ => panic!("opcode incorrecto"),
        }
    }

    #[test]
    fn roundtrip_continuos() {
        match rt(ControlCmd::SetVolume { track: 2, value: 0.37 }) {
            ControlCmd::SetVolume { track: 2, value } => assert!((value - 0.37).abs() < 1e-4),
            _ => panic!("opcode incorrecto"),
        }
        match rt(ControlCmd::SetPan { track: 1, value: -0.5 }) {
            ControlCmd::SetPan { value, .. } => assert!((value + 0.5).abs() < 2e-4),
            _ => panic!("opcode incorrecto"),
        }
        match rt(ControlCmd::SetTempo { bpm: 128.5 }) {
            ControlCmd::SetTempo { bpm } => assert!((bpm - 128.5).abs() < 0.05),
            _ => panic!("opcode incorrecto"),
        }
        // Fader de 14 bits: exacto en los extremos.
        assert_eq!(rt(ControlCmd::SetMasterVol { value: 1.0 }), ControlCmd::SetMasterVol { value: 1.0 });
        assert_eq!(rt(ControlCmd::SetGrooveMasterVol { value: 1.0 }), ControlCmd::SetGrooveMasterVol { value: 1.0 });
    }

    #[test]
    fn roundtrip_deltas_negativos() {
        assert_eq!(rt(ControlCmd::SelectTrack { delta: -3 }), ControlCmd::SelectTrack { delta: -3 });
        assert_eq!(rt(ControlCmd::SelectScene { delta: 1 }), ControlCmd::SelectScene { delta: 1 });
        assert_eq!(rt(ControlCmd::CajaBpm { delta: -128 }), ControlCmd::CajaBpm { delta: -128 });
    }

    #[test]
    fn roundtrip_plugin_params() {
        // El valor viaja cuantizado a 14 bits: 0.75 no es exacto (error ~1.5e-5).
        match rt(ControlCmd::SetPluginParam { track: 3, param_idx: 5, value: 0.75 }) {
            ControlCmd::SetPluginParam { track, param_idx, value } => {
                assert_eq!(track, 3);
                assert_eq!(param_idx, 5);
                assert!((value - 0.75).abs() < 1e-4);
            }
            _ => panic!("opcode incorrecto"),
        }
        assert_eq!(rt(ControlCmd::SelectPluginParam { track: TRACK_CURSOR, param_idx: 7 }),
                   ControlCmd::SelectPluginParam { track: TRACK_CURSOR, param_idx: 7 });
    }


    #[test]
    fn roundtrip_pitch_y_mod() {
        assert_eq!(rt(ControlCmd::LivePitchBend { track: TRACK_CURSOR, value: 8192 }),
                   ControlCmd::LivePitchBend { track: TRACK_CURSOR, value: 8192 });
        assert_eq!(rt(ControlCmd::LivePitchBend { track: 2, value: 16383 }),
                   ControlCmd::LivePitchBend { track: 2, value: 16383 });
        assert_eq!(rt(ControlCmd::LiveModWheel { track: TRACK_CURSOR, value: 127 }),
                   ControlCmd::LiveModWheel { track: TRACK_CURSOR, value: 127 });
        assert_eq!(rt(ControlCmd::LiveSustain { track: TRACK_CURSOR, value: 127 }),
                   ControlCmd::LiveSustain { track: TRACK_CURSOR, value: 127 });
        assert_eq!(rt(ControlCmd::LiveSustain { track: 1, value: 0 }),
                   ControlCmd::LiveSustain { track: 1, value: 0 });
    }

    #[test]
    fn roundtrip_notas_en_vivo() {
        for c in [
            ControlCmd::LiveNoteOn { track: 3, note: 60, vel: 127 },
            ControlCmd::LiveNoteOn { track: TRACK_CURSOR, note: 0, vel: 1 },
            ControlCmd::LiveNoteOff { track: TRACK_CURSOR, note: 127 },
        ] { assert_eq!(rt(c), c); }
    }

    #[test]
    fn roundtrip_pads_en_vivo() {
        assert_eq!(rt(ControlCmd::LivePadOn { pad: 15, vel: 127 }),
                   ControlCmd::LivePadOn { pad: 15, vel: 127 });
        assert_eq!(rt(ControlCmd::LivePadOff { pad: 3 }),
                   ControlCmd::LivePadOff { pad: 3 });
    }

    #[test]
    fn opcode_desconocido_es_none() {
        assert!(ControlCmd::decode(0xFFFF_FFFF).is_none());
    }
}

#[cfg(test)]
mod create_clip_tests {
    use super::*;

    #[test]
    fn roundtrip_create_clip() {
        for c in [
            ControlCmd::CreateClip { track: 0, scene: 0, bars: 2 },
            ControlCmd::CreateClip { track: 31, scene: 31, bars: 8 },
            ControlCmd::CreateClip { track: TRACK_CURSOR, scene: SCENE_CURSOR, bars: 4 },
        ] {
            assert_eq!(ControlCmd::decode(c.encode()), Some(c));
        }
    }

    #[test]
    fn el_opcode_nuevo_no_choca_con_los_viejos() {
        // 36 debe seguir siendo decodificable y distinto de LaunchClip.
        let a = ControlCmd::CreateClip { track: 3, scene: 5, bars: 2 }.encode();
        let b = ControlCmd::LaunchClip { track: 3, scene: 5 }.encode();
        assert_ne!(a, b);
        assert_eq!(a >> 26, 36);
    }
}
