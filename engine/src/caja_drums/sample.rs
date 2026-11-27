// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! Muestra decodificada estéreo. Versión mínima del plugin/src/sample.rs
//! sin dependencias de nih-plug (solo Arc<[f32]>).

pub struct Sample {
    pub left: Vec<f32>,
    pub right: Option<Vec<f32>>,
}

impl Sample {
    pub fn is_empty(&self) -> bool { self.left.is_empty() }
    pub fn len(&self) -> usize { self.left.len() }

    /// Overview mono para UI (no RT). Devuelve picos absolutos 0..1.
    pub fn overview(&self, bins: usize) -> Vec<f32> {
        let n = bins.max(1);
        let mut out = vec![0.0f32; n];
        let frames = self.left.len();
        if frames == 0 {
            return out;
        }
        let right = self.right.as_deref();
        for b in 0..n {
            let a = b * frames / n;
            let e = (((b + 1) * frames / n).max(a + 1)).min(frames);
            let mut m = 0.0f32;
            for i in a..e {
                let mut v = self.left[i].abs();
                if let Some(r) = right {
                    if let Some(rv) = r.get(i) {
                        v = v.max(rv.abs());
                    }
                }
                if v > m {
                    m = v;
                }
            }
            out[b] = m.clamp(0.0, 1.0);
        }
        out
    }
}
