// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! Clip y su máquina de estados (Session View de Bitwig).
use serde::{Deserialize, Serialize};
use crate::{PPQ, TICKS_PER_BAR};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipState {
    Empty,
    Triggered,   // esperando al siguiente punto de quantize para arrancar
    Playing,
    Stopping,    // fade-out o wait-to-end antes de detenerse
    Recording,   // grabando audio o MIDI en slot vacío
    Overdubbing, // grabando sobre clip existente
    RecQueued,   // armado: espera la frontera de cuantización para grabar
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LaunchMode { Trigger, Toggle, Gate, Legato, Repeat }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FollowAction { None, Next, Previous, First, Last, Stop, Random, Again, JumpTo(u8) }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Quantize {
    Immediate,
    Eighth, Quarter, Half,
    One, Two, Four, Eight, // en barras
    /// Subdivisiones finas: 1/16 y 1/32 de negra.
    Sixteenth, ThirtySecond,
}

impl Quantize {
    /// Rejilla en ticks (480 PPQ). Immediate = 0 (sin espera).
    pub fn ticks(self) -> u64 {
        match self {
            Quantize::Immediate => 0,
            Quantize::ThirtySecond => (PPQ / 8) as u64,
            Quantize::Sixteenth => (PPQ / 4) as u64,
            Quantize::Eighth => (PPQ / 2) as u64,
            Quantize::Quarter => PPQ as u64,
            Quantize::Half => (PPQ * 2) as u64,
            Quantize::One => TICKS_PER_BAR,
            Quantize::Two => TICKS_PER_BAR * 2,
            Quantize::Four => TICKS_PER_BAR * 4,
            Quantize::Eight => TICKS_PER_BAR * 8,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Clip {
    pub state: ClipState,
    pub launch_mode: LaunchMode,
    pub follow_action: FollowAction,
    pub follow_bars: u8,   // después de N barras → follow_action (0 = usar loop_count)
    pub quantize: Quantize,
    pub loop_bars: u8,     // longitud del loop
    pub loop_count: u8,    // veces a repetir (0 = infinito)
    pub name: String,
    pub color: [u8; 3],
    pub audio_path: Option<String>,
    pub midi_notes: Vec<MidiNote>, // Fase 2: reproducción real
    pub probability: f32, // 0.0–1.0 (Inspector de Bitwig)
    /// Offset (segundos) donde empieza el loop dentro del archivo. 0 = principio.
    #[serde(default)]
    pub loop_start_sec: f32,
}

impl Default for Clip {
    fn default() -> Self {
        Clip {
            state: ClipState::Empty,
            launch_mode: LaunchMode::Trigger,
            follow_action: FollowAction::None,
            follow_bars: 0,
            quantize: Quantize::One,
            loop_bars: 1,
            loop_count: 0,
            name: String::new(),
            color: [120, 120, 120],
            audio_path: None,
            midi_notes: Vec::new(),
            probability: 1.0,
            loop_start_sec: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct MidiNote {
    pub tick_start: u32,
    pub tick_dur: u32,
    pub note: u8,
    pub vel: u8,
    pub channel: u8,
    pub occurrence: u8, // en qué repetición del loop suena (0 = siempre) — Fase 4 con Caja
    pub probability: f32,
}

#[cfg(test)]
mod quantize_fino_tests {
    use super::*;

    #[test]
    fn la_rejilla_fina_es_mas_corta_que_la_octava() {
        assert_eq!(Quantize::ThirtySecond.ticks(), (PPQ / 8) as u64);
        assert_eq!(Quantize::Sixteenth.ticks(), (PPQ / 4) as u64);
        assert!(Quantize::ThirtySecond.ticks() < Quantize::Sixteenth.ticks());
        assert!(Quantize::Sixteenth.ticks() < Quantize::Eighth.ticks());
    }

    #[test]
    fn immediate_sigue_sin_espera() {
        assert_eq!(Quantize::Immediate.ticks(), 0);
    }

    #[test]
    fn las_rejillas_de_compas_no_cambian() {
        assert_eq!(Quantize::One.ticks(), TICKS_PER_BAR);
        assert_eq!(Quantize::Eight.ticks(), TICKS_PER_BAR * 8);
    }
}

#[cfg(test)]
mod follow_jump_tests {
    use super::*;

    #[test]
    fn jump_to_conserva_la_escena_destino() {
        assert_eq!(FollowAction::JumpTo(7), FollowAction::JumpTo(7));
        assert_ne!(FollowAction::JumpTo(7), FollowAction::JumpTo(8));
        assert_ne!(FollowAction::JumpTo(7), FollowAction::Next);
    }

    #[test]
    fn jump_to_serializa_y_vuelve() {
        let c = Clip { follow_action: FollowAction::JumpTo(12), ..Default::default() };
        let j = serde_json::to_string(&c).unwrap();
        let back: Clip = serde_json::from_str(&j).unwrap();
        assert_eq!(back.follow_action, FollowAction::JumpTo(12));
    }

    #[test]
    fn los_proyectos_antiguos_siguen_cargando() {
        // Los FollowAction sin dato se serializan por nombre: añadir JumpTo
        // no cambia ese formato.
        let c = Clip { follow_action: FollowAction::Next, ..Default::default() };
        let j = serde_json::to_string(&c).unwrap();
        assert!(j.contains("\"Next\""));
        let back: Clip = serde_json::from_str(&j).unwrap();
        assert_eq!(back.follow_action, FollowAction::Next);
    }
}
