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
use lanzadera_engine::{loader, SessionShared};
use lanzadera_ui::SessionView;

struct LanzaderaApp {
    shared: Arc<SessionShared>,
    view: SessionView,
    _audio: Option<AudioHandle>,
    _conns: Vec<lanzadera_controller::MidiConn>,
    _leds: Option<lanzadera_controller::leds::LedHandle>,
    _kl_leds: Option<lanzadera_controller::keylab_leds::KeylabLedHandle>,
    #[allow(dead_code)] // El hilo vive hasta que el programa termina
        taps: Vec<Instant>,
}

impl eframe::App for LanzaderaApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Reinicio del stream cuando cambia el set de plugins CLAP.
        if self.shared.audio_restart.swap(false, std::sync::atomic::Ordering::Relaxed) {
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

        // Notas grabadas por el audio → clip MIDI (overdub).
        let mut rec: Vec<lanzadera_core::MidiNote> = Vec::new();
        let mut rec_track = 0usize;
        while let Some(n) = self.shared.rec_rx.pop() {
            rec_track = n.track as usize;
            rec.push(lanzadera_core::MidiNote {
                tick_start: n.tick, tick_dur: n.dur, note: n.note, vel: n.vel,
                channel: 0, occurrence: 0, probability: 1.0,
            });
        }
        if !rec.is_empty() {
            let (t, s, _) = { self.shared.state.lock().unwrap().record_target() };
            let count = rec.len();
            self.shared.edit(|st| {
                if let Some(tr) = st.tracks.get_mut(t) {
                    if let Some(Some(c)) = tr.clips.get_mut(s) {
                        for n in &rec { if n.note > 0 || n.vel > 0 { c.midi_notes.push(*n); } }
                    }
                }
            });
            let _ = rec_track;
            self.shared.sync_midi(t, s);
            self.view.msg = format!("● {} notas grabadas en Pista {} · Escena {}", count, t + 1, s + 1);
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
    for track in 0..lanzadera_core::MAX_TRACKS {
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
            shared: app_shared, view, _audio: audio, _conns: conns,
            _leds: leds, _kl_leds: kl_leds, taps: Vec::new(),
        }))
    })) {
        eprintln!("[lanzadera] error de GUI: {}", e);
        std::process::exit(1);
    }
}
