// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! lanzadera — standalone de escritorio (Fase 2: GUI egui + audio real).
//! Uso:
//!   cargo run -p lanzadera                 → GUI + audio (ALSA/PipeWire)
//!   cargo run -p lanzadera -- --midi       → + KeyLab 49 / MK3 + LEDs MK3
//!   cargo run -p lanzadera -- proyecto.lanzadera
//!   cargo run -p lanzadera --features jack → JACK primero
use std::sync::Arc;
use std::time::{Duration, Instant};

use lanzadera_core::control::ControlCmd;
use lanzadera_engine::audio_io::{self, AudioHandle};
use lanzadera_engine::midi_ctl::{self, MidiProfile, MidiRequest, MidiStatus};
use lanzadera_engine::{loader, SessionShared};
use lanzadera_ui::SessionView;

/// Controlador conectado desde Preferencias (o reconectado automáticamente).
struct MidiEntry {
    profile: MidiProfile,
    port: String,
    #[allow(dead_code)] // solo mantiene viva la conexión MIDI mientras exista la entrada
    conns: Vec<lanzadera_controller::MidiConn>,
    leds: Option<lanzadera_controller::leds::LedHandle>,
}

struct LanzaderaApp {
    midi_entries: Vec<MidiEntry>,
    last_midi_scan: Instant,
    /// Con `--midi` la conexión es la de siempre; la reconexión automática no se mezcla.
    legacy_midi: bool,
    shared: Arc<SessionShared>,
    view: SessionView,
    _audio: Option<AudioHandle>,
    _conns: Vec<lanzadera_controller::MidiConn>,
    _leds: Option<lanzadera_controller::leds::LedHandle>,
    _kl_leds: Option<lanzadera_controller::keylab_leds::KeylabLedHandle>,
    #[allow(dead_code)] // El hilo vive hasta que el programa termina
        taps: Vec<Instant>,
}

impl LanzaderaApp {
    fn midi_connect(&mut self, profile: MidiProfile, port: &str) -> String {
        if self.legacy_midi {
            let lp = port.to_lowercase();
            let es_puerto_legacy = lp.contains("keylab") || lp.contains("maschine")
                || lp.contains("mikro") || lp.contains("virtual raw midi");
            if matches!(profile, MidiProfile::KeyLab | MidiProfile::Mk3) || es_puerto_legacy {
                return "Ya conectado con --midi; no lo conectes tambien desde Preferencias".to_string();
            }
        }
        if profile == MidiProfile::Generic {
            let lp = port.to_lowercase();
            let integrado = self.midi_entries.iter()
                .any(|e| matches!(e.profile, MidiProfile::KeyLab | MidiProfile::Mk3));
            if integrado && (lp.contains("keylab") || lp.contains("maschine")
                || lp.contains("mikro") || lp.contains("virtual raw midi")) {
                return "Ese puerto ya lo usa el controlador integrado (KeyLab/MK3)".to_string();
            }
        }
        if self.midi_entries.iter().any(|e| e.port == port) {
            return format!("{} ya está conectado", profile.label());
        }
        let sink = self.shared.ctrl_rx.clone();
        let shared = self.shared.clone();
        let result: Result<MidiEntry, String> = match profile {
            MidiProfile::Generic => lanzadera_controller::connect_generic(port, sink)
                .map(|c| MidiEntry { profile, port: port.to_string(), conns: vec![c], leds: None })
                .map_err(|e| e.to_string()),
            MidiProfile::KeyLab => lanzadera_controller::connect_keylab(sink, shared)
                .map(|cs| MidiEntry { profile, port: port.to_string(), conns: cs, leds: None })
                .map_err(|e| e.to_string()),
            MidiProfile::Mk3 => lanzadera_controller::connect_mk3(port, sink, shared.clone())
                .map_err(|e| e.to_string())
                .map(|c| {
                    // Los LED salen por el puerto de salida homónimo.
                    let hint = if port.contains("Maschine Mikro") { "Maschine Mikro" } else { port };
                    let leds = lanzadera_controller::leds::spawn_mk3_leds(hint, shared).ok();
                    MidiEntry { profile, port: port.to_string(), conns: vec![c], leds }
                }),
        };
        match result {
            Ok(e) => { self.midi_entries.push(e); format!("Conectado: {} → {}", profile.label(), port) }
            Err(e) => format!("⚠ {}: {}", profile.label(), e),
        }
    }

    fn midi_disconnect(&mut self, profile: MidiProfile, port: &str) {
        if let Some(i) = self.midi_entries.iter().position(|e| e.profile == profile && e.port == port) {
            let e = self.midi_entries.remove(i);
            if let Some(l) = &e.leds { l.stop(); }
        }
    }

    /// Atiende las peticiones de Preferencias, suelta los controladores desenchufados y
    /// reconecta los marcados como automáticos. Sin peticiones ni automáticos no hace nada.
    fn service_midi(&mut self) {
        let reqs = midi_ctl::take_requests();
        let auto = lanzadera_core::config::get().midi_auto;
        let periodic = !self.legacy_midi && !auto.is_empty()
            && self.last_midi_scan.elapsed() >= Duration::from_secs(3);
        if reqs.is_empty() && !periodic { return; }
        self.last_midi_scan = Instant::now();

        let mut msg = String::new();
        for r in reqs {
            match r {
                MidiRequest::Rescan => {}
                MidiRequest::Connect { profile, port } => { msg = self.midi_connect(profile, &port); }
                MidiRequest::Disconnect { profile, port } => {
                    self.midi_disconnect(profile, &port);
                    msg = format!("Desconectado: {}", port);
                }
            }
        }
        let ports = lanzadera_controller::list_input_ports();

        // Puertos que desaparecieron (se desenchufó el aparato).
        let mut i = 0;
        while i < self.midi_entries.len() {
            let e = &self.midi_entries[i];
            let present = match e.profile {
                MidiProfile::KeyLab => ports.iter().any(|p| p.to_lowercase().contains("keylab")),
                _ => ports.iter().any(|p| p == &e.port),
            };
            if present { i += 1; } else {
                let gone = self.midi_entries.remove(i);
                if let Some(l) = &gone.leds { l.stop(); }
                msg = format!("Se desconectó {}", gone.port);
            }
        }
        // Autoconexión de los aparatos recordados.
        if !self.legacy_midi {
            for a in &auto {
                let Some(p) = MidiProfile::from_id(&a.profile) else { continue };
                let present = ports.iter().any(|x| x == &a.port);
                let connected = self.midi_entries.iter().any(|e| e.profile == p && e.port == a.port);
                if present && !connected {
                    let m = self.midi_connect(p, &a.port);
                    if msg.is_empty() { msg = m; }
                }
            }
        }
        midi_ctl::set_status(MidiStatus {
            ports,
            connected: self.midi_entries.iter().map(|e| (e.profile, e.port.clone())).collect(),
            msg,
        });
    }
}

impl eframe::App for LanzaderaApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Reinicio del stream cuando cambia el set de plugins CLAP.
        self.service_midi();
        // Captura del estado de los plugins (guardar proyecto, autoguardado).
        if lanzadera_engine::state_ctl::take_capture_request() {
            if let Some(a) = self._audio.as_mut() { a.capture_plugin_states(&self.shared); }
        }
        if self.shared.audio_restart.swap(false, std::sync::atomic::Ordering::Relaxed) {
            // Antes de descargar los plugins se guarda su estado, para que al volver a
            // cargarlos (p. ej. al añadir un FX) no pierdan lo que el usuario ajustó.
            // Al abrir/crear un proyecto NO se captura: pisaría el estado recién cargado.
            if !lanzadera_engine::state_ctl::take_skip_capture() {
                if let Some(a) = self._audio.as_mut() { a.capture_plugin_states(&self.shared); }
            }
            self._audio = None; // desactiva los plugins viejos ordenadamente
            match audio_io::start(self.shared.clone()) {
                Ok(h) => { self._audio = Some(h); self.view.msg = "Audio reiniciado".into(); }
                Err(e) => { self.view.msg = format!("⚠ Audio: {}", e); }
            }
        }
        // Bomba de comandos MIDI (controller → UI), con tap tempo real.
        while let Some(raw) = self.shared.ctrl_rx.pop() {
            if let Some(cmd) = ControlCmd::decode(raw) {
                match cmd {
                    ControlCmd::ToggleMetronome => {
                        self.shared.edit(|st| st.metronome = !st.metronome);
                    }
                    ControlCmd::TapTempo => {
                        let now = Instant::now();
                        if let Some(last) = self.taps.last() {
                            if now.duration_since(*last) < Duration::from_millis(120) { continue; }
                        }
                        self.taps.retain(|t| now.duration_since(*t) < Duration::from_secs(3));
                        self.taps.push(now);
                        if self.taps.len() >= 2 {
                            let total: Duration = self.taps.windows(2)
                                .map(|w| w[1].duration_since(w[0])).sum();
                            let avg = total.as_secs_f64() / (self.taps.len() - 1) as f64;
                            if avg > 0.0 {
                                let bpm = (60.0 / avg).clamp(20.0, 666.0);
                                self.shared.apply_control(ControlCmd::SetTempo { bpm: bpm as f32 });
                                self.view.msg = format!("Tap: {:.1} BPM", bpm);
                            }
                        }
                    }
                    c => self.shared.apply_control(c),
                }
            }
        }
        // Las operaciones de presets son costosas: ejecutarlas aquí, fuera
        // del callback MIDI de midir. Los bitmasks agrupan peticiones pendientes
        // y evitan perder pulsaciones de distintos slots entre frames.
        let (load_mask, save_mask) = self.shared.take_mk3_preset_requests();
        for slot in 0..8 {
            let bit = 1u8 << slot;
            if load_mask & bit != 0 {
                match self.shared.load_groove_preset_slot(slot) {
                    Ok(()) => {
                        println!("[MK3] Preset cargado desde slot {}", slot + 1);
                        self.view.msg = format!("Preset cargado desde slot {}", slot + 1);
                    }
                    Err(e) => {
                        println!("[MK3] No se pudo cargar slot {}: {}", slot + 1, e);
                        self.view.msg = format!("No se pudo cargar slot {}: {}", slot + 1, e);
                    }
                }
            }
            if save_mask & bit != 0 {
                match self.shared.save_groove_preset_slot(slot) {
                    Ok(()) => {
                        println!("[MK3] Preset guardado en slot {}", slot + 1);
                        self.view.msg = format!("Preset guardado en slot {}", slot + 1);
                    }
                    Err(e) => {
                        println!("[MK3] No se pudo guardar slot {}: {}", slot + 1, e);
                        self.view.msg = format!("No se pudo guardar slot {}: {}", slot + 1, e);
                    }
                }
            }
        }

        // Notas grabadas por el audio → clip MIDI (overdub). Cada nota trae su pista y su
        // escena: el destino NO se deduce del cursor actual, que puede haber cambiado
        // (p. ej. al lanzar otro clip mientras se grababa).
        let mut groups: Vec<((usize, usize), Vec<lanzadera_core::MidiNote>)> = Vec::new();
        let mut finals: Vec<(usize, usize, u32)> = Vec::new();
        while let Some(n) = self.shared.rec_rx.pop() {
            let key = (n.track as usize, n.scene as usize);
            if n.note == 0 && n.vel == 0 && n.dur == 0 {
                finals.push((key.0, key.1, n.tick));
                continue;
            }
            let note = lanzadera_core::MidiNote {
                tick_start: n.tick, tick_dur: n.dur, note: n.note, vel: n.vel,
                channel: 0, occurrence: 0, probability: 1.0,
            };
            match groups.iter_mut().find(|(k, _)| *k == key) {
                Some((_, v)) => v.push(note),
                None => groups.push((key, vec![note])),
            }
        }
        for ((t, s), notes) in groups {
            let count = notes.len();
            let (tmin, tmax) = notes.iter().fold((u32::MAX, 0u32), |(a, b), n| (a.min(n.tick_start), b.max(n.tick_start + n.tick_dur.max(1))));
            let loop_bars = {
                let st = self.shared.state.lock().unwrap();
                st.tracks.get(t).and_then(|tr| tr.clips.get(s)).and_then(|c| c.as_ref()).map(|c| c.loop_bars).unwrap_or(0)
            };
            self.shared.edit(|st| {
                let overdub = st.overdub;
                if let Some(tr) = st.tracks.get_mut(t) {
                    if let Some(Some(c)) = tr.clips.get_mut(s) {
                        for n in &notes { if n.note > 0 || n.vel > 0 { c.midi_notes.push(*n); } }
                        if !overdub && tmax > 0 {
                            let bars = ((tmax as u64 + lanzadera_core::TICKS_PER_BAR - 1) / lanzadera_core::TICKS_PER_BAR).max(1) as u8;
                            c.loop_bars = c.loop_bars.max(bars);
                        }
                    }
                }
            });
            self.shared.sync_midi(t, s);
            eprintln!("[rec] {} notas → pista {} escena {} (ticks {}..{}, loop de {} compases)",
                count, t + 1, s + 1, tmin, tmax, loop_bars);
            self.view.msg = format!("● {} notas grabadas en Pista {} · Escena {}", count, t + 1, s + 1);
        }
        for (t, s, end_tick) in finals {
            self.shared.edit(|st| {
                if st.overdub { return; }
                if let Some(tr) = st.tracks.get_mut(t) {
                    if let Some(Some(c)) = tr.clips.get_mut(s) {
                        let max_note_end = c.midi_notes.iter().map(|n| n.tick_start.saturating_add(n.tick_dur.max(1))).max().unwrap_or(0);
                        let end = end_tick.max(max_note_end);
                        let bars = ((end as u64 + lanzadera_core::TICKS_PER_BAR - 1) / lanzadera_core::TICKS_PER_BAR).max(1) as u8;
                        c.loop_bars = c.loop_bars.max(bars);
                    }
                }
            });
            self.shared.sync_midi(t, s);
        }
        // GUI de plugins: la UI encola la pista y aqui se atiende, en el hilo
        // principal y con las instancias vivas.
        #[cfg(feature = "clap")]
        if let Some(a) = &mut self._audio { a.poll_gui_requests(); }

        self.view.show(ctx, &self.shared, self._audio.is_some());

        // Errores de carga/GUI de plugins (antes solo salian por stderr).
        #[cfg(feature = "clap")]
        if let Some(m) = lanzadera_engine::clap_gui::take_notice() { self.view.msg = m; }
        // Las GUI VST3 de Linux solo se dibujan si el host les da cuerda cada
        // frame; sin esto egui duerme hasta el siguiente evento de raton.
        #[cfg(feature = "clap")]
        {
            // Timers CLAP precisos, con el cadencia VST3 (16 ms) como techo.
            let timer_delay = lanzadera_engine::clap_services::next_wakeup();
            if lanzadera_engine::clap_gui::any_open() || timer_delay.is_some() {
                let delay = timer_delay
                    .unwrap_or(Duration::from_millis(16))
                    .min(Duration::from_millis(16));
                ctx.request_repaint_after(delay);
            }
    if lanzadera_engine::clap_gui::has_pending_request() { ctx.request_repaint(); }

    // request_restart del plugin → tu mecanismo existente de reinicio.
    for track in 0..lanzadera_engine::clap_gui::MAX_PLUGIN_SLOTS {
        if lanzadera_engine::clap_services::take_restart(track) {
            self.shared.audio_restart.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

        // ── GUI nativa de plugins CLAP (Fase 3H) ─────────────────────────
        #[cfg(feature = "clap")]
        for track in self.view.take_native_gui_requests() {
            lanzadera_engine::clap_gui::request_toggle(track);
        }

    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.shared.loader_stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(l) = &self._leds { l.stop(); }
        for e in &self.midi_entries { if let Some(l) = &e.leds { l.stop(); } }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let midi = args.iter().any(|a| a == "--midi");
    let kl_leds_on = args.iter().any(|a| a == "--keylab-leds");
    let project = args.iter().find(|a| !a.starts_with("--")).cloned();

    let shared = SessionShared::new();
    if let Some(p) = &project {
        if let Err(e) = shared.load_from_file(p) { eprintln!("[lanzadera] no se pudo cargar {}: {}", p, e); }
    }

    // Audio real (si falla, la GUI sigue funcionando para editar el proyecto).
    let (audio, audio_err) = match audio_io::start(shared.clone()) {
        Ok(h) => (Some(h), String::new()),
        Err(e) => (None, e),
    };
    let rate = audio.as_ref().map(|a| a.sample_rate).unwrap_or(48_000);
    let _loader = loader::spawn(shared.clone(), rate);

    // Controladores opcionales.
    let mut conns = Vec::new();
    let mut leds = None;
    let mut kl_leds = None;
    if midi {
        println!("Puertos MIDI: {:?}", lanzadera_controller::list_input_ports());
        let sink = shared.ctrl_rx.clone();
        match lanzadera_controller::connect_keylab(sink.clone(), shared.clone()) {
            Ok(cs) => {
                println!("KeyLab 49 conectado ({} puertos)", cs.len());
                conns.extend(cs);
                

                if kl_leds_on {
                    match lanzadera_controller::keylab_leds::spawn("KeyLab Essential 49 DAW", shared.clone()) {
                        Ok(h) => { println!("LEDs KeyLab activos (experimental)"); kl_leds = Some(h); }
                        Err(e) => println!("LEDs KeyLab: {}", e),
                    }
                }
            }
            Err(e) => println!("KeyLab 49: {}", e),
        }
        for hint in ["Maschine Mikro", "Virtual Raw MIDI"] {
            match lanzadera_controller::connect_mk3(hint, sink.clone(), shared.clone()) {
                Ok(c) => {
                    println!("MK3 conectado vía '{}'", hint);
                    conns.push(c);
                    match lanzadera_controller::leds::spawn_mk3_leds(hint, shared.clone()) {
                        Ok(l) => { println!("LEDs MK3 activos"); leds = Some(l); }
                        Err(e) => println!("LEDs MK3: {}", e),
                    }
                    break;
                }
                Err(e) => println!("MK3 ({}): {}", hint, e),
            }
        }
    }

    let mut view = SessionView::default();
    if let Some(p) = &project { view.project_path = p.clone(); }
    if !audio_err.is_empty() { view.msg = format!("⚠ Audio: {}", audio_err); }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_title("lanzadera — clip launcher"),
        ..Default::default()
    };
    let app_shared = shared.clone();
    if let Err(e) = eframe::run_native("lanzadera", options, Box::new(move |cc| {
        lanzadera_ui::theme::apply(&cc.egui_ctx);

        // ── Paso 1 CLAP: wakeup de egui + identidad del hilo principal ──
        #[cfg(feature = "clap")]
        {
            let wake_ctx = cc.egui_ctx.clone();
            lanzadera_engine::clap_services::set_waker(move || wake_ctx.request_repaint());
            lanzadera_engine::clap_services::mark_main_thread();
        }

        Ok(Box::new(LanzaderaApp {
            midi_entries: Vec::new(),
            last_midi_scan: Instant::now().checked_sub(Duration::from_secs(10)).unwrap_or_else(Instant::now),
            legacy_midi: midi,
            shared: app_shared, view, _audio: audio, _conns: conns,
            _leds: leds, _kl_leds: kl_leds, taps: Vec::new(),
        }))
    })) {
        eprintln!("[lanzadera] error de GUI: {}", e);
        std::process::exit(1);
    }
}
