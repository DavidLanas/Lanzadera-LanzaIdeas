//! ClipEngine — scheduling quantizado del launcher en el hilo de audio.
//! Sin locks, sin reservas, sin I/O. Los datos llegan por &SessionAudio
//! (TripleBuffer) y por el Spsc<u32> (ControlCmd codificados). Los eventos de
//! salida se escriben en un Vec PRE-RESERVADO por el llamador: si se llena se
//! descartan, jamás crece aquí (ver tests/no_alloc.rs).
//!
//! Fase 1: emite StateChange. NoteOn/NoteOff llegan en Fase 2 con el almacén
//! POD de notas MIDI. Gate/Repeat necesitan press/release del controlador
//! (Fase 2); Trigger, Toggle y Legato ya están implementados.
use lanzadera_core::audio_state::{ClipMeta, SessionAudio};
use lanzadera_core::control::ControlCmd;
use lanzadera_core::{ClipState, FollowAction, LaunchMode, Quantize, Spsc, MAX_SCENES, MAX_TRACKS, PPQ, TICKS_PER_BAR};

#[derive(Clone, Copy, Debug)]
pub enum ClipEvent {
    NoteOn { track: u8, note: u8, vel: u8, channel: u8, offset: u32 },
    NoteOff { track: u8, note: u8, channel: u8, offset: u32 },
    StateChange { track: u8, scene: u8, new_state: ClipState },
}

#[derive(Clone, Copy, Debug)]
pub struct ActiveClip {
    pub track: u8,
    pub scene: u8,
    pub playhead: u32,  // ticks dentro del loop
    pub loop_ticks: u32,
    pub repeat_left: u8, // u8::MAX = infinito
    pub state: ClipState,
    pub launch_mode: LaunchMode, // Fase 2: Gate/Repeat lo leen del clip activo
    follow: FollowAction,
    /// true durante el tick de arranque: el playhead no se incrementa
    /// hasta el tick siguiente, así la primera pasada dispara el tick 0.
    just_started: bool,
}

#[derive(Clone, Copy)]
enum TrigAction { Launch, Stop }

#[derive(Clone, Copy)]
struct PendingTrigger { scene: u8, action: TrigAction, quantize_ticks: u64, restart: bool }

pub struct ClipEngine {
    active: [Option<ActiveClip>; MAX_TRACKS],
    pending: [Option<PendingTrigger>; MAX_TRACKS],
    global_ticks: u64, // ticks acumulados desde play (480 PPQ)
    sample_rate: f64,
    samples_per_tick: f64,
    fractional: f64,
    last_tempo: f64,
    rng: u64, // xorshift64 para probability / Random (determinista, sin alloc)
    /// Ciclo de Groove Machine en ticks (0 = ignorar). Los lanzamientos
    /// cuantizados se alinean a este ciclo para no caer a mitad de frase.
    pub groove_loop_ticks: u32,
    /// Escena lanzada con el transporte en marcha: en esta frontera el reloj
    /// global vuelve a 0 (todo —clips y Groove— reinicia en 1.1).
    restart_q: Option<u64>,
    /// Durante un reinicio de escena, start_clip NO es idempotente.
    force_restart: bool,
}

impl ClipEngine {
    pub fn new(sample_rate: f64) -> Self {
        ClipEngine {
            active: [None; MAX_TRACKS],
            pending: [None; MAX_TRACKS],
            global_ticks: 0,
            sample_rate,
            samples_per_tick: sample_rate * 60.0 / (120.0 * PPQ as f64),
            fractional: 0.0,
            last_tempo: 120.0,
            rng: 0x9E37_79B9_7F4A_7C15,
            groove_loop_ticks: 0,
            restart_q: None,
            force_restart: false,
        }
    }

    /// Actualiza el ciclo de Groove Machine (llamar cada bloque desde el audio).
    pub fn set_groove_loop_ticks(&mut self, ticks: u32) {
        self.groove_loop_ticks = ticks;
    }

    /// Llamado desde el audio callback. Sin locks ni reservas.
    pub fn process(&mut self, audio: &SessionAudio, ctrl_rx: &Spsc, out_events: &mut Vec<ClipEvent>, frame_count: u32) {
        self.drain_ctrl(ctrl_rx, audio, out_events);
        self.advance(frame_count, audio, out_events);
    }

    /// Quantize efectivo: si Groove está activo, el lanzamiento espera un
    /// múltiplo común del quantize del clip y del loop de batería.
    /// Así un clip con quantize de 1 compás no arranca a mitad del ciclo
    /// de 2 compases de la batería.
    fn effective_quantize_ticks(&self, q: Quantize, clip_loop_ticks: u64) -> u64 {
        let qt = q.ticks();
        if qt == 0 {
            return 0;
        }
        let gt = self.groove_loop_ticks as u64;
        if gt == 0 || clip_loop_ticks == 0 {
            return qt;
        }
        // El clip solo necesita caer en un múltiplo de MCD(loop_clip, loop_groove):
        // un clip de 1 compás sobre una batería de 8 compases no espera 8;
        // uno de 4 sobre 8 espera como mucho 4. (Antes: MCM(q, groove) siempre.)
        let period = Self::gcd_u64(clip_loop_ticks, gt);
        let g = Self::gcd_u64(qt, period);
        qt / g * period
    }

    fn gcd_u64(mut a: u64, mut b: u64) -> u64 {
        while b != 0 {
            let t = b;
            b = a % b;
            a = t;
        }
        a.max(1)
    }

    pub fn active(&self, t: usize) -> Option<ActiveClip> { self.active[t] }

    /// Detiene de inmediato el clip activo de una pista y emite el evento
    /// correspondiente.
    ///
    /// Usado por la grabación para imponer "una pista, una voz" sin depender
    /// de que ControlCmd::Record viaje por trig_rx. Inspirado en la regla de
    /// session view de MAGDA (GPL-3).
    pub fn stop_track_immediate(&mut self, t: usize, out: &mut Vec<ClipEvent>) {
        if t >= MAX_TRACKS { return; }

        // Cancelar cualquier trigger pendiente en esa pista.
        self.pending[t] = None;

        // Apagar el clip activo, si lo había.
        if let Some(a) = self.active[t].take() {
            Self::emit(out, ClipEvent::StateChange {
                track: t as u8,
                scene: a.scene,
                new_state: ClipState::Empty,
            });
        }
    }
    pub fn global_ticks(&self) -> u64 { self.global_ticks }
    /// Posición del reloj en ticks, con la fracción de tick acumulada.
    pub fn position_ticks(&self) -> f64 {
        self.global_ticks as f64 + self.fractional / self.samples_per_tick.max(1.0)
    }
    pub fn set_seed(&mut self, seed: u64) { self.rng = if seed == 0 { 1 } else { seed }; }

    /// Reinicia el reloj de transporte a 1.1 (tick 0). Llamar al parar
    /// el transporte para que el siguiente Play arranque alineado con el
    /// downbeat y con Groove Machine.
    pub fn reset_transport(&mut self) {
        self.global_ticks = 0;
        self.fractional = 0.0;
    }

    /// Samples por tick al tempo actual (para el render de audio).
    pub fn samples_per_tick(&self) -> f64 { self.samples_per_tick }

    /// Fase exacta del clip activo en frames (playhead en ticks + fracción
    /// de tick acumulada). La usa el callback de audio por cada frame.
    pub fn phase_frames(&self, t: usize) -> Option<f64> {
        self.active[t].map(|a| a.playhead as f64 * self.samples_per_tick + self.fractional)
    }

    /// Los ticks NO dependen del tempo (480 PPQ fijo); el tempo solo cambia
    /// samples_per_tick. Se mantiene la firma del spec por compatibilidad.
    pub fn quantize_ticks(&self, q: Quantize, _tempo: f64) -> u64 { q.ticks() }

    // ── internals ──────────────────────────────────────────────────────────

    fn drain_ctrl(&mut self, rx: &Spsc, audio: &SessionAudio, out: &mut Vec<ClipEvent>) {
        while let Some(raw) = rx.pop() {
            let Some(cmd) = ControlCmd::decode(raw) else { continue };
            match cmd {
                ControlCmd::LaunchClip { track, scene } => self.queue_launch(audio, track as usize, scene as usize, out),
                ControlCmd::StopClip { track } => self.queue_stop(audio, track as usize, out),
                ControlCmd::LaunchScene { scene } => {
                    let s = scene as usize;
                    // Frontera común: MCM de los quantize de los clips de la fila,
                    // así todos disparan en el MISMO downbeat.
                    let mut common_q: u64 = 0;
                    for t in 0..audio.track_count as usize {
                        let meta = audio.clip_meta(t, s);
                        if !meta.exists { continue; }
                        let qt = meta.quantize.ticks();
                        if qt == 0 { continue; }
                        common_q = if common_q == 0 { qt } else {
                            let g = Self::gcd_u64(common_q, qt);
                            common_q / g * qt
                        };
                    }
                    // Con el transporte en marcha, lanzar una escena reinicia TODO
                    // desde 1.1: en la frontera el reloj global vuelve a 0 y Groove
                    // Machine (que sigue ese reloj) arranca su ciclo de nuevo.
                    let restart = audio.playing;
                    if restart {
                        if common_q == 0 { self.global_ticks = 0; } else { self.restart_q = Some(common_q); }
                    }
                    for t in 0..audio.track_count as usize {
                        let meta = audio.clip_meta(t, s);
                        if !meta.exists { continue; }
                        let ahora = common_q == 0 || (meta.quantize == Quantize::Immediate && !restart);
                        if ahora {
                            self.force_restart = restart;
                            self.start_clip(t, s, meta, out);
                            self.force_restart = false;
                        } else {
                            if let Some(p) = self.pending[t] {
                                if matches!(p.action, TrigAction::Launch) && p.scene as usize != s {
                                    Self::emit(out, ClipEvent::StateChange {
                                        track: t as u8, scene: p.scene, new_state: ClipState::Empty,
                                    });
                                }
                            }
                            self.pending[t] = Some(PendingTrigger {
                                scene: s as u8, action: TrigAction::Launch,
                                quantize_ticks: common_q, restart,
                            });
                            Self::emit(out, ClipEvent::StateChange {
                                track: t as u8, scene: s as u8, new_state: ClipState::Triggered,
                            });
                        }
                    }
                }
                ControlCmd::ToggleMetronome => {
                    // Este comando se maneja en el UI thread, no en el audio thread
                }
                ControlCmd::StopAll => {
                    // Parar todo y volver el transporte a 1.1 del compás.
                    // Así el siguiente LaunchScene / LaunchClip entra
                    // perfectamente alineado (comportamiento esperado en vivo).
                    for t in 0..audio.track_count as usize {
                        self.pending[t] = None;
                        if let Some(a) = self.active[t].take() {
                            Self::emit(out, ClipEvent::StateChange {
                                track: t as u8, scene: a.scene, new_state: ClipState::Empty,
                            });
                        }
                    }
                    self.restart_q = None;
                    self.global_ticks = 0;
                    self.fractional = 0.0;
                }
                _ => {} // el resto de comandos los resuelve el hilo de UI
            }
        }
    }

    fn queue_launch(&mut self, audio: &SessionAudio, t: usize, s: usize, out: &mut Vec<ClipEvent>) {
        if t >= MAX_TRACKS || s >= MAX_SCENES { return; }
        let meta = audio.clip_meta(t, s);
        if !meta.exists { return; }
        // Probabilidad del clip (Inspector): 0.0 = nunca, 1.0 = siempre.
        if meta.probability < 1.0 && self.next_rand() > meta.probability { return; }
        // Toggle: relanzar el clip que ya suena en la pista lo detiene.
        if meta.launch_mode == LaunchMode::Toggle {
            if let Some(a) = self.active[t] {
                if a.scene as usize == s { self.queue_stop(audio, t, out); return; }
            }
        }
        if meta.quantize == Quantize::Immediate {
            self.start_clip(t, s, meta, out);
        } else {
            // Solo puede haber un lanzamiento en cola por pista: si había otro
            // clip esperando, se cancela y su celda vuelve a reposo.
            if let Some(p) = self.pending[t] {
                if matches!(p.action, TrigAction::Launch) && p.scene as usize != s {
                    Self::emit(out, ClipEvent::StateChange {
                        track: t as u8, scene: p.scene, new_state: ClipState::Empty,
                    });
                }
            }
            self.pending[t] = Some(PendingTrigger { scene: s as u8, action: TrigAction::Launch, quantize_ticks: self.effective_quantize_ticks(meta.quantize, meta.loop_bars.max(1) as u64 * TICKS_PER_BAR), restart: false });
            Self::emit(out, ClipEvent::StateChange { track: t as u8, scene: s as u8, new_state: ClipState::Triggered });
        }
    }

    fn queue_stop(&mut self, audio: &SessionAudio, t: usize, out: &mut Vec<ClipEvent>) {
        if t >= MAX_TRACKS { return; }
        let scene = self.active[t].map(|a| a.scene).unwrap_or(0);
        if audio.quantize == Quantize::Immediate {
            self.pending[t] = None;
            if self.active[t].take().is_some() {
                Self::emit(out, ClipEvent::StateChange { track: t as u8, scene, new_state: ClipState::Empty });
            }
        } else if self.active[t].is_some() || self.pending[t].is_some() {
            self.pending[t] = Some(PendingTrigger { scene, action: TrigAction::Stop, quantize_ticks: self.effective_quantize_ticks(audio.quantize, 0), restart: false });
            Self::emit(out, ClipEvent::StateChange { track: t as u8, scene, new_state: ClipState::Stopping });
        }
    }

    fn start_clip(&mut self, t: usize, s: usize, meta: ClipMeta, out: &mut Vec<ClipEvent>) {
        // Si el mismo clip ya está sonando, no reiniciamos el playhead ni
        // emitimos otro StateChange::Playing. Evita el "doble ataque" al
        // abrir el piano roll o al re-lanzar por error (estilo Bitwig/Ableton:
        // re-trigger solo si el usuario lo pide o LaunchMode lo exige).
        if let Some(prev) = self.active[t] {
            if prev.scene as usize == s && prev.state == ClipState::Playing && !self.force_restart {
                self.pending[t] = None;
                return;
            }
        }

        let loop_ticks = meta.loop_bars.max(1) as u32 * TICKS_PER_BAR as u32;
        // Fase del playhead al arrancar:
        // - Legato: hereda la fase del clip anterior de la pista.
        // - Quantize Immediate: se engancha al reloj global (mismo grid que
        //   Groove Machine). Si arrancara siempre en 0 a mitad de compás,
        //   el clip y la batería quedarían permanentemente desfasados.
        // - Quantize de barra/subdivision: arranca en 0 porque ya esperamos
        //   la frontera; el downbeat del clip coincide con el del transporte.
        let mut playhead = 0u32;
        if meta.launch_mode == LaunchMode::Legato {
            if let Some(prev) = self.active[t] {
                playhead = prev.playhead % loop_ticks;
            } else if loop_ticks > 0 {
                playhead = (self.global_ticks % loop_ticks as u64) as u32;
            }
        } else if meta.quantize == Quantize::Immediate && loop_ticks > 0 {
            playhead = (self.global_ticks % loop_ticks as u64) as u32;
        }
        // follow_bars tiene prioridad sobre loop_count (interpretación del spec §5.2/§7.1).
        let lb = meta.loop_bars.max(1) as u32;
        let repeat_left = if meta.follow_bars > 0 {
            ((meta.follow_bars as u32 + lb - 1) / lb).clamp(1, 254) as u8
        } else if meta.loop_count == 0 { u8::MAX } else { meta.loop_count };
        // Una sola ranura sonando por pista: el clip desplazado debe avisar
        // que se apagó, o su celda y su LED quedan encendidos para siempre.
        if let Some(prev) = self.active[t] {
            if prev.scene as usize != s {
                Self::emit(out, ClipEvent::StateChange {
                    track: t as u8, scene: prev.scene, new_state: ClipState::Empty,
                });
            }
        }
        self.active[t] = Some(ActiveClip {
            track: t as u8, scene: s as u8, playhead, loop_ticks, repeat_left,
            state: ClipState::Playing, launch_mode: meta.launch_mode, follow: meta.follow_action,,
        just_started: true,
});
        self.pending[t] = None;
        Self::emit(out, ClipEvent::StateChange { track: t as u8, scene: s as u8, new_state: ClipState::Playing });
    }

    fn advance(&mut self, frame_count: u32, audio: &SessionAudio, out: &mut Vec<ClipEvent>) {
        if !audio.playing { return; }
        if audio.tempo != self.last_tempo && audio.tempo > 0.0 {
            self.last_tempo = audio.tempo;
            self.samples_per_tick = self.sample_rate * 60.0 / (audio.tempo * PPQ as f64);
        }
        for _ in 0..frame_count {
            self.fractional += 1.0;
            while self.fractional >= self.samples_per_tick {
                self.fractional -= self.samples_per_tick;
                self.global_ticks = self.global_ticks.wrapping_add(1);
                self.on_tick(audio, out);
            }
        }
    }

    fn on_tick(&mut self, audio: &SessionAudio, out: &mut Vec<ClipEvent>) {
        // Reinicio de escena: en la frontera el reloj vuelve a 1.1.
        if let Some(q) = self.restart_q {
            if q == 0 || self.global_ticks % q == 0 {
                self.restart_q = None;
                self.global_ticks = 0;
            }
        }
        let now = self.global_ticks;
        for t in 0..audio.track_count as usize {
            // 1) Triggers pendientes → aplicar en el punto de quantize
            //    (alineado al ciclo de Groove si está activo).
            if let Some(p) = self.pending[t] {
                let q = p.quantize_ticks;
                if q == 0 || now % q == 0 {
                    self.pending[t] = None;
                    match p.action {
                        TrigAction::Launch => {
                            let meta = audio.clip_meta(t, p.scene as usize);
                            if meta.exists {
                                self.force_restart = p.restart;
                                self.start_clip(t, p.scene as usize, meta, out);
                                self.force_restart = false;
                            }
                        }
                        TrigAction::Stop => {
                            if let Some(a) = self.active[t].take() {
                                Self::emit(out, ClipEvent::StateChange { track: t as u8, scene: a.scene, new_state: ClipState::Empty });
                            }
                        }
                    }
                }
            }
            // 2) Avanzar el playhead del clip activo (sample-accurate vía ticks).
            if let Some(mut a) = self.active[t] {
                // Si el usuario cambia los compases en el Inspector, el clip que
                // suena lo adopta al instante (antes quedaba con la longitud con
                // que arrancó hasta el siguiente stop global).
                let m = audio.clip_meta(t, a.scene as usize);
                if m.exists {
                    let nl = m.loop_bars.max(1) as u32 * TICKS_PER_BAR as u32;
                    if nl != a.loop_ticks {
                        a.loop_ticks = nl;
                        a.playhead %= nl;
                    }
                }
                // El tick de arranque no incrementa el playhead: el MidiVoice se crea
                // con playhead 0 y debe disparar el tick 0 en la primera pasada.
                if a.just_started {
                    a.just_started = false;
                    self.active[t] = Some(a);
                    continue;
                }
                a.playhead += 1;
                if a.playhead >= a.loop_ticks {
                    a.playhead = 0;
                    if a.repeat_left != u8::MAX { a.repeat_left = a.repeat_left.saturating_sub(1); }
                    if a.repeat_left == 0 {
                        // Fin de repeticiones → follow action.
                        let target = self.follow_target(audio, &a);
                        self.active[t] = Some(a); // fase disponible si el follow es Legato
                        match target {
                            Some(ns) if audio.clip_meta(t, ns).exists => {
                                let m = audio.clip_meta(t, ns);
                                self.start_clip(t, ns, m, out);
                            }
                            _ => {
                                self.active[t] = None;
                                Self::emit(out, ClipEvent::StateChange { track: t as u8, scene: a.scene, new_state: ClipState::Empty });
                            }
                        }
                        continue;
                    }
                }
                self.active[t] = Some(a);
            }
        }
    }

    fn follow_target(&mut self, audio: &SessionAudio, a: &ActiveClip) -> Option<usize> {
        let t = a.track as usize;
        let s = a.scene as usize;
        match a.follow {
            FollowAction::None | FollowAction::Stop => None,
            FollowAction::Again => Some(s),
            FollowAction::Next => {
                let n = (s + 1) % MAX_SCENES;
                if audio.clip_meta(t, n).exists { Some(n) } else { None }
            }
            FollowAction::Previous => {
                if s == 0 { return None; }
                if audio.clip_meta(t, s - 1).exists { Some(s - 1) } else { None }
            }
            FollowAction::First => Some(0),
            FollowAction::Last => Some(MAX_SCENES - 1),
            FollowAction::JumpTo(dest) => {
                // Salto a una escena concreta. Si está vacía no hay destino,
                // igual que Next/Previous.
                let d = (dest as usize).min(MAX_SCENES - 1);
                if audio.clip_meta(t, d).exists { Some(d) } else { None }
            }
            FollowAction::Random => {
                // Cuenta los clips existentes y elige uno con el RNG interno.
                let mut count = 0usize;
                for sc in 0..MAX_SCENES { if audio.clip_meta(t, sc).exists { count += 1; } }
                if count == 0 { return None; }
                let pick = ((self.next_rand() * count as f32) as usize).min(count - 1);
                let mut idx = 0usize;
                for sc in 0..MAX_SCENES {
                    if audio.clip_meta(t, sc).exists {
                        if idx == pick { return Some(sc); }
                        idx += 1;
                    }
                }
                None
            }
        }
    }

    fn next_rand(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13; x ^= x >> 7; x ^= x << 17;
        self.rng = x;
        ((x >> 40) as f32) / ((1u32 << 24) as f32)
    }

    fn emit(out: &mut Vec<ClipEvent>, ev: ClipEvent) {
        // El llamador pre-reserva capacidad; si se llena se descarta el evento
        // (nunca se reserva memoria en el hilo de audio).
        if out.len() < out.capacity() { out.push(ev); }
    }
}
