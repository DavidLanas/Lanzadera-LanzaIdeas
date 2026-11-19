//! EQ de 3 bandas + HPF/LPF por pista (biquads RBJ, Direct Form I).
//! Coeficientes recalculados solo cuando cambian los parámetros.
//! El audio thread no reserva memoria: todo es aritmética sobre arrays fijos.
use lanzadera_core::MAX_TRACKS;

#[derive(Clone, Copy, Default)]
pub struct Biquad { b0: f32, b1: f32, b2: f32, a1: f32, a2: f32, x1: f32, x2: f32, y1: f32, y2: f32 }

impl Biquad {
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1; self.x1 = x; self.y2 = self.y1; self.y1 = y;
        y
    }
    fn norm(&mut self, b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) {
        let inv = 1.0 / a0;
        self.b0 = b0 * inv; self.b1 = b1 * inv; self.b2 = b2 * inv;
        self.a1 = a1 * inv; self.a2 = a2 * inv;
    }
    pub fn peaking(&mut self, sr: f32, f0: f32, q: f32, gain_db: f32) {
        let a = 10.0f32.powf(gain_db / 40.0);
        let w = 2.0 * std::f32::consts::PI * (f0 / sr).clamp(1e-5, 0.45);
        let (s, c) = w.sin_cos();
        let al = s / (2.0 * q);
        self.norm(1.0 + al * a, -2.0 * c, 1.0 - al * a, 1.0 + al / a, -2.0 * c, 1.0 - al / a);
    }
    pub fn highpass(&mut self, sr: f32, f0: f32, q: f32) {
        let w = 2.0 * std::f32::consts::PI * (f0 / sr).clamp(1e-5, 0.45);
        let (s, c) = w.sin_cos();
        let al = s / (2.0 * q);
        self.norm((1.0 + c) * 0.5, -(1.0 + c), (1.0 + c) * 0.5, 1.0 + al, -2.0 * c, 1.0 - al);
    }
    pub fn lowpass(&mut self, sr: f32, f0: f32, q: f32) {
        let w = 2.0 * std::f32::consts::PI * (f0 / sr).clamp(1e-5, 0.45);
        let (s, c) = w.sin_cos();
        let al = s / (2.0 * q);
        self.norm((1.0 - c) * 0.5, 1.0 - c, (1.0 - c) * 0.5, 1.0 + al, -2.0 * c, 1.0 - al);
    }
}

#[derive(Clone, Copy, PartialEq)]
pub struct EqParams {
    pub sr: f32, pub low: f32, pub mid: f32, pub high: f32,
    pub mid_f: f32, pub mid_q: f32,
    pub hpf_on: bool, pub lpf_on: bool, pub hpf_hz: f32, pub lpf_hz: f32,
}

/// Orden de bandas: 0 HPF · 1 low 120 Hz · 2 mid (freq/Q variables) · 3 high 6 kHz · 4 LPF.
pub struct TrackEq { ch: [[Biquad; 5]; 2], on: [bool; 5], cache: Option<EqParams> }

impl TrackEq {
    pub fn new() -> Self { TrackEq { ch: [[Biquad::default(); 5]; 2], on: [false; 5], cache: None } }

    pub fn update(&mut self, p: EqParams) {
        if self.cache == Some(p) { return; }
        self.cache = Some(p);
        self.on = [p.hpf_on, p.low.abs() > 0.01, p.mid.abs() > 0.01, p.high.abs() > 0.01, p.lpf_on];
        let mid_f = p.mid_f.clamp(80.0, 12000.0);
        let mid_q = p.mid_q.clamp(0.15, 8.0);
        for c in 0..2 {
            self.ch[c][0].highpass(p.sr, p.hpf_hz.clamp(20.0, 2000.0), 0.707);
            self.ch[c][1].peaking(p.sr, 120.0, 0.7, p.low.clamp(-15.0, 15.0));
            self.ch[c][2].peaking(p.sr, mid_f, mid_q, p.mid.clamp(-15.0, 15.0));
            self.ch[c][3].peaking(p.sr, 6000.0, 0.7, p.high.clamp(-15.0, 15.0));
            self.ch[c][4].lowpass(p.sr, p.lpf_hz.clamp(200.0, 20000.0), 0.707);
        }
    }

    #[inline]
    pub fn process(&mut self, c: usize, x: f32) -> f32 {
        let mut y = x;
        for i in 0..5 { if self.on[i] { y = self.ch[c][i].process(y); } }
        y
    }
}

pub struct EqBank { pub bands: [TrackEq; MAX_TRACKS] }
impl EqBank {
    pub fn new() -> Self { EqBank { bands: std::array::from_fn(|_| TrackEq::new()) } }
}
