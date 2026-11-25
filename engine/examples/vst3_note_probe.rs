// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! Comprueba que las notas llegan a un instrumento VST3 por `vst3_slot::process`
//! (el camino que usa el callback de audio) y que sale audio.
//!   cargo run -p lanzadera-engine --features vst3 --example vst3_note_probe -- /ruta/Synth.vst3
use lanzadera_engine::plugin_events::PluginEvents;
use lanzadera_engine::vst3_slot;

fn peak(slot: &vst3_slot::Vst3Slot) -> f32 {
    slot.buffers.outputs.first().map(|b| b.channels.iter()
        .flat_map(|c| c.iter()).fold(0.0f32, |m, &x| m.max(x.abs()))).unwrap_or(0.0)
}

fn main() {
    let path = std::env::args().nth(1).expect("ruta .vst3");
    let mut slot = vst3_slot::load(&path, 48000, 4096).expect("load");
    println!("cargado: {} (instrumento={})", slot.name, slot.is_instrument);
    let fr: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(512);
    let mut q = PluginEvents::new();
    // `process` ahora recibe también el audio de entrada (un instrumento no lo usa).
    let silence = vec![0.0f32; fr];

    // 1) silencio sin notas
    q.clear();
    let mut silent = 0.0f32;
    for _ in 0..8 { assert!(vst3_slot::process(&mut slot, q.as_slice(), fr, &silence, &silence)); silent = silent.max(peak(&slot)); }
    assert_eq!(slot.buffers.outputs[0].channels[0].len(), fr, "el plugin debe procesar solo `frames` muestras");
    println!("pico sin notas      = {silent:.5}");

    // 2) nota encendida
    let mut on = 0.0f32;
    for b in 0..40 {
        q.clear();
        if b == 0 { q.note_on(0, 60, 100, 0); }
        assert!(vst3_slot::process(&mut slot, q.as_slice(), fr, &silence, &silence));
        on = on.max(peak(&slot));
    }
    println!("pico con nota 60    = {on:.5}");

    // 3) nota apagada: la cola debe decaer a ~0
    let mut tail = 0.0f32;
    for b in 0..400 {
        q.clear();
        if b == 0 { q.note_off(0, 60, 0); }
        assert!(vst3_slot::process(&mut slot, q.as_slice(), fr, &silence, &silence));
        tail = peak(&slot);
    }
    println!("pico tras note_off  = {tail:.5}");
    println!("{}", if on > 0.001 && on > silent * 10.0 { "RESULTADO: OK (el VST3 suena con notas)" } else { "RESULTADO: FALLO" });
}
