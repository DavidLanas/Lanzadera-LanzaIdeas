#!/usr/bin/env python3
# fix_loteB_hotfix1.py — 5 correcciones seguras sobre loteA+loteB.
# Uso: python3 fix_loteB_hotfix1.py   (desde la raíz del repo)
import re, sys
from pathlib import Path

def apply(path, edits):
    p = Path(path); src = p.read_text(encoding="utf-8")
    for pat, repl in edits:
        new, n = re.subn(pat, repl, src, count=1)
        if n != 1:
            print(f"✗ {path}: ancla no encontrada: {pat[:70]}..."); sys.exit(1)
        src = new
    p.write_text(src, encoding="utf-8"); print(f"✓ {path}")

# F1 · Tap tempo: descartar taps duplicados (<120 ms = mismo golpe duplicado)
apply("app/src/main.rs", [
    (r"(ControlCmd::TapTempo => \{\n([ \t]*)let now = Instant::now\(\);\n)",
     r"\1\2if let Some(last) = self.taps.last() {\n"
     r"\2    if now.duration_since(*last) < Duration::from_millis(120) { continue; }\n"
     r"\2}\n"),
    # F2 · Un puerto MIDI = un solo bridge (evita Mk3+Generic / duplicados)
    (r"if self\.midi_entries\.iter\(\)\.any\(\|e\| e\.profile == profile && e\.port == port\) \{",
     "if self.midi_entries.iter().any(|e| e.port == port) {"),
    (r"(fn midi_connect\(&mut self, profile: MidiProfile, port: &str\) -> String \{\n)",
     r"\1        if self.legacy_midi && matches!(profile, MidiProfile::KeyLab | MidiProfile::Mk3) {\n"
     r"            return \"Ya conectado con --midi; no lo conectes tambien desde Preferencias\".to_string();\n"
     r"        }\n"),
])

# F3 · Abrir proyecto nunca hereda 'recording' del JSON
apply("engine/src/shared.rs", [
    (r"(s\.playing = false;\n)([ \t]*self\.edit\(\|st\| \*st = s\);)",
     r"\1        s.recording = false;\n\2"),
])

# F4 · Borrar pista / Stop-All apagan SIEMPRE sinte y sustain (panic)
apply("engine/src/audio_io.rs", [
    (r"(for i in 0\.\.kill_n \{\n([ \t]*)self\.engine\.stop_track_immediate\(kill\[i\], &mut self\.events\);\n)",
     r"\1\2let kt = kill[i];\n"
     r"\2self.synth.all_off(kt as u8);\n"
     r"\2self.sustain_down[kt] = false;\n"
     r"\2self.sustained[kt] = [false; 128];\n"
     r"\2#[cfg(feature = \"clap\")]\n"
     r"\2if self.has_plugin(kt) { self.pev[kt].midi3(0, 0xB0, 123, 0); }\n"),
    (r"(if STOP_ALL_REQ\.swap\(false, std::sync::atomic::Ordering::Relaxed\) \{\n([ \t]*)self\.engine\.reset_transport\(\);\n[ \t]*self\.caja_scheduler\.reset\(\);\n)",
     r"\1\2for t in 0..n {\n"
     r"\2    self.synth.all_off(t as u8);\n"
     r"\2    self.sustain_down[t] = false;\n"
     r"\2    self.sustained[t] = [false; 128];\n"
     r"\2}\n"),
    # F5 · Publicar el pico del Groove (post groove_master, pre master general)
    (r"(peaks: \[f32; MAX_TRACKS \+ 1\],\n)", r"\1    groove_peak: f32,\n"),
    (r"(peaks: \[0\.0; MAX_TRACKS \+ 1\],\n)", r"\1        groove_peak: 0.0,\n"),
    (r"([ \t]*)ml \+= self\.caja_buf\[0\]\[f\] \* audio\.groove_master;\n[ \t]*mr \+= self\.caja_buf\[1\]\[f\] \* audio\.groove_master;",
     r"\1let gl = self.caja_buf[0][f] * audio.groove_master;\n"
     r"\1let gr = self.caja_buf[1][f] * audio.groove_master;\n"
     r"\1ml += gl;\n\1mr += gr;\n"
     r"\1let gp = gl.abs().max(gr.abs());\n"
     r"\1if gp > self.groove_peak { self.groove_peak = gp; }"),
    (r"(shared\.rt\.set_peak\(MAX_TRACKS, self\.peaks\[MAX_TRACKS\]\);\n[ \t]*self\.peaks\[MAX_TRACKS\] = 0\.0;\n)",
     r"\1        shared.rt.set_groove_peak(self.groove_peak);\n"
     r"        self.groove_peak = 0.0;\n"),
])

apply("engine/src/rt.rs", [
    (r"(groove_step:[ \t]*AtomicU8,[^\n]*\n)", r"\1    groove_peak: AtomicU32, // f32 bits; pico del Groove\n"),
    (r"(groove_step:[ \t]*AtomicU8::new\(255\),\n)", r"\1            groove_peak: AtomicU32::new(0),\n"),
    (r"(pub fn groove_step\(&self\) -> u8 \{ self\.groove_step\.load\(Ordering::Relaxed\) \}\n)",
     r"\1    pub fn set_groove_peak(&self, v: f32) { self.groove_peak.store(v.to_bits(), Ordering::Relaxed); }\n"
     r"    pub fn groove_peak(&self) -> f32 { f32::from_bits(self.groove_peak.load(Ordering::Relaxed)) }\n"),
])
print("Hotfix 1 aplicado. Ahora: cargo test --workspace --features clap,vst3")