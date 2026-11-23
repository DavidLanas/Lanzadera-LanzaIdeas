//! Cola de eventos de plugin NEUTRAL (Fase G5a).
//!
//! POD, sin alloc, sin tipos de ningun formato. El callback de audio escribe
//! aqui (note_on / midi3 / param); un unico traductor al final del bloque
//! convierte la cola al formato del plugin. Asi los sitios que generan
//! eventos no saben si detras hay CLAP o VST3.
//!
//! El traductor CLAP vive aqui abajo bajo `cfg(feature = "clap")`, para que
//! audio_io.rs no necesite importar event_types ni Pckn/Match.

pub const MAX_PLUGIN_EVENTS: usize = 512;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EvKind { NoteOn, NoteOff, Midi3, Param }

#[derive(Clone, Copy)]
pub struct PluginEv {
    pub frame: u32,
    pub kind: EvKind,
    /// NoteOn/Off: nota. Midi3: status byte.
    pub a: u8,
    /// NoteOn: velocidad. Midi3: data1.
    pub b: u8,
    /// NoteOn/Off: canal. Midi3: data2.
    pub c: u8,
    /// Param: id del parametro en el plugin.
    pub id: u32,
    /// Param: valor ya desnormalizado (plain).
    pub plain: f64,
}

pub struct PluginEvents {
    evs: [PluginEv; MAX_PLUGIN_EVENTS],
    len: usize,
}

impl PluginEvents {
    pub fn new() -> Self {
        PluginEvents {
            evs: [PluginEv {
                frame: 0, kind: EvKind::NoteOn, a: 0, b: 0, c: 0, id: 0, plain: 0.0,
            }; MAX_PLUGIN_EVENTS],
            len: 0,
        }
    }

    #[inline] pub fn clear(&mut self) { self.len = 0; }
    #[inline] pub fn len(&self) -> usize { self.len }
    #[inline] pub fn is_empty(&self) -> bool { self.len == 0 }
    #[inline] pub fn as_slice(&self) -> &[PluginEv] { &self.evs[..self.len] }

    /// Añade un evento ya construido (respeta el tope duro, sin alloc).
    #[inline] pub fn push_ev(&mut self, e: PluginEv) { self.push(e); }

    /// Ordena por frame de forma ESTABLE y sin alloc (inserción): los eventos del
    /// mismo frame conservan su orden (NoteOff antes que NoteOn, etc.).
    pub fn sort_by_frame(&mut self) {
        for i in 1..self.len {
            let mut j = i;
            while j > 0 && self.evs[j - 1].frame > self.evs[j].frame {
                self.evs.swap(j - 1, j);
                j -= 1;
            }
        }
    }

    #[inline]
    fn push(&mut self, e: PluginEv) {
        // Tope duro: nunca crece, nunca asigna. Igual que el criterio que ya
        // usaba audio_io con MAX_EVENTS.
        if self.len < MAX_PLUGIN_EVENTS {
            self.evs[self.len] = e;
            self.len += 1;
        }
    }

    #[inline]
    pub fn note_on(&mut self, frame: u32, note: u8, vel: u8, ch: u8) {
        self.push(PluginEv { frame, kind: EvKind::NoteOn, a: note, b: vel, c: ch, id: 0, plain: 0.0 });
    }

    #[inline]
    pub fn note_off(&mut self, frame: u32, note: u8, ch: u8) {
        self.push(PluginEv { frame, kind: EvKind::NoteOff, a: note, b: 0, c: ch, id: 0, plain: 0.0 });
    }

    /// Mensaje MIDI de 3 bytes crudo (pitch bend, CC, etc).
    #[inline]
    pub fn midi3(&mut self, frame: u32, status: u8, d1: u8, d2: u8) {
        self.push(PluginEv { frame, kind: EvKind::Midi3, a: status, b: d1, c: d2, id: 0, plain: 0.0 });
    }

    /// Cambio de parametro: id del plugin + valor plain ya desnormalizado.
    #[inline]
    pub fn param(&mut self, frame: u32, id: u32, plain: f64) {
        self.push(PluginEv { frame, kind: EvKind::Param, a: 0, b: 0, c: 0, id, plain });
    }
}

/// Unico punto que conoce el formato CLAP. Vuelca la cola neutral en el
/// EventBuffer ya reservado: sin alloc, y en el mismo orden de frame en que
/// se generaron los eventos (CLAP exige orden temporal).
#[cfg(feature = "clap")]
pub fn flush_clap(q: &PluginEvents, out: &mut clack_host::events::io::EventBuffer) {
    use clack_host::events::event_types::{MidiEvent, NoteOffEvent, NoteOnEvent, ParamValueEvent};
    use clack_host::events::{Match, Pckn};
    use clack_host::prelude::ClapId;

    out.clear();
    for e in q.as_slice() {
        match e.kind {
            EvKind::NoteOn => {
                // Pckn de nota: puerto 0, canal real, key = nota, note_id = nota.
                // Identico a lo que hacia audio_io::note() antes de G5a.
                let p = Pckn::new(0u16, e.c as u16, e.a as u16, e.a as u32);
                let _ = out.push(&NoteOnEvent::new(e.frame, p, e.b as f64 / 127.0));
            }
            EvKind::NoteOff => {
                let p = Pckn::new(0u16, e.c as u16, e.a as u16, e.a as u32);
                let _ = out.push(&NoteOffEvent::new(e.frame, p, 0.0));
            }
            EvKind::Midi3 => {
                let _ = out.push(&MidiEvent::new(e.frame, 0u16, [e.a, e.b, e.c]));
            }
            EvKind::Param => {
                if let Some(pid) = ClapId::from_raw(e.id) {
                    // Pckn neutro: sin automatizacion por puerto/canal/tecla/nota.
                    let _ = out.push(&ParamValueEvent::new(
                        e.frame,
                        pid,
                        Pckn::new(Match::All, Match::All, Match::All, Match::All),
                        e.plain,
                    ));
                }
            }
        }
    }
}

/// Inverso de `flush_clap`: lee los eventos que un Note FX CLAP escribió en su cola
/// de salida (notas y MIDI crudo) y los vuelca en la cola neutral. Sin alloc.
#[cfg(feature = "clap")]
pub fn read_clap(src: &clack_host::events::io::EventBuffer, dst: &mut PluginEvents) {
    use clack_host::events::spaces::CoreEventSpace;
    use clack_host::prelude::Event; // trae header() al scope
    for ev in src.iter() {
        match ev.as_core_event() {
            Some(CoreEventSpace::NoteOn(n)) => {
                let p = n.pckn();
                let vel = ((n.velocity() * 127.0).round() as i32).clamp(1, 127) as u8;
                dst.note_on(n.header().time(), p.raw_key().clamp(0, 127) as u8, vel, p.raw_channel().clamp(0, 15) as u8);
            }
            Some(CoreEventSpace::NoteOff(n)) => {
                let p = n.pckn();
                dst.note_off(n.header().time(), p.raw_key().clamp(0, 127) as u8, p.raw_channel().clamp(0, 15) as u8);
            }
            Some(CoreEventSpace::Midi(m)) => {
                let d = m.data();
                dst.midi3(m.header().time(), d[0], d[1], d[2]);
            }
            _ => {}
        }
    }
}
