//! Notas MIDI para el hilo de audio: datos POD tras Arc (sin locks ni alloc)
//! y scheduling por tick con playhead del ClipEngine (480 PPQ).
use std::sync::Arc;
use lanzadera_core::MidiNote;

/// Notas de un clip, ordenadas por tick_start (las prepara el hilo de UI).
pub struct MidiNoteData { pub notes: Vec<MidiNote> }

#[derive(Clone, Copy, Debug)]
pub enum MidiMsg {
    On { note: u8, vel: u8, ch: u8 },
    Off { note: u8, ch: u8 },
}

/// Buffer fijo de mensajes por frame (nunca crece).
pub struct MidiOut { pub msgs: [MidiMsg; 64], pub len: usize }
impl MidiOut {
    pub fn new() -> Self { MidiOut { msgs: [MidiMsg::Off { note: 0, ch: 0 }; 64], len: 0 } }
    pub fn clear(&mut self) { self.len = 0; }
    pub fn push(&mut self, m: MidiMsg) { if self.len < 64 { self.msgs[self.len] = m; self.len += 1; } }
}

/// Reproductor de notas de una pista. `held` = notas sonando (nota 255 = libre).
pub struct MidiVoice {
    data: Arc<MidiNoteData>,
    last_tick: u32,
    loops_done: u32,
    started: bool,
    held: [(u8, u8, u32); 32],
    rng: u64,
}

impl MidiVoice {
    /// `start_tick` = playhead inicial (Legato entra a mitad de loop).
    pub fn new(data: Arc<MidiNoteData>, loop_ticks: u32, start_tick: u32) -> Self {
        let lt = loop_ticks.max(1);
        MidiVoice {
            data, last_tick: (start_tick + lt - 1) % lt, loops_done: 0, started: false,
            held: [(255, 0, 0); 32], rng: 0x2545_F491_4F6C_DD1D,
        }
    }

    /// Avanza hasta `tick` emitiendo on/off. Normalmente 1 tick por frame.
    pub fn advance(&mut self, tick: u32, loop_ticks: u32, out: &mut MidiOut) {
        let lt = loop_ticks.max(1);
        if tick == self.last_tick { return; }
        let steps = if tick > self.last_tick { tick - self.last_tick } else { (lt - self.last_tick) + tick };
        let mut t = self.last_tick;
        for _ in 0..steps.min(lt) {
            t = (t + 1) % lt;
            // La primera pasada por el tick 0 es la repetición 1: solo se
            // cuentan los wraps POSTERIORES (si no, occurrence=2 sonaría ya
            // en la primera vuelta).
            if t == 0 && self.started { self.loops_done = self.loops_done.wrapping_add(1); }
            self.tick_once(t, out);
            self.started = true;
        }
        self.last_tick = tick;
    }

    fn tick_once(&mut self, t: u32, out: &mut MidiOut) {
        // 1) Note-offs de las notas sostenidas.
        for h in self.held.iter_mut() {
            if h.0 != 255 {
                if h.2 <= 1 { out.push(MidiMsg::Off { note: h.0, ch: h.1 }); h.0 = 255; }
                else { h.2 -= 1; }
            }
        }
        // 2) Note-ons de este tick.
        // Clonamos el Arc (solo refcount atómico, SIN alloc) para iterar las
        // notas sin prestar `self`: así podemos mutar self.held y self.rng
        // dentro del bucle (evita E0502 y mantiene el camino RT sin reservas).
        let data = Arc::clone(&self.data);
        for n in data.notes.iter() {
            if n.tick_start != t { continue; }
            // Occurrence (0 = siempre; N = suena en la repetición N del ciclo de 4).
            if n.occurrence > 0 && (self.loops_done % 4) + 1 != n.occurrence as u32 { continue; }
            if n.probability < 1.0 && self.rand() > n.probability { continue; }
            // Retrigger de la misma nota: corta la anterior antes de la nueva.
            if let Some(h) = self.held.iter_mut().find(|h| h.0 == n.note && h.1 == n.channel) {
                out.push(MidiMsg::Off { note: h.0, ch: h.1 });
                *h = (n.note, n.channel, n.tick_dur.max(1));
            } else if let Some(h) = self.held.iter_mut().find(|h| h.0 == 255) {
                *h = (n.note, n.channel, n.tick_dur.max(1));
            }
            out.push(MidiMsg::On { note: n.note, vel: n.vel, ch: n.channel });
        }
    }

    pub fn all_off(&mut self, out: &mut MidiOut) {
        for h in self.held.iter_mut() {
            if h.0 != 255 { out.push(MidiMsg::Off { note: h.0, ch: h.1 }); h.0 = 255; }
        }
    }

    fn rand(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13; x ^= x >> 7; x ^= x << 17;
        self.rng = x;
        ((x >> 40) as f32) / ((1u32 << 24) as f32)
    }
}
