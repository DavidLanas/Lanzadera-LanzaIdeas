//! Edicion de la muestra de un clip (lote C3). El archivo original no se toca:
//! cada operacion escribe un WAV nuevo junto al actual y recarga el clip por el
//! loader (decodifica + picos + publica). Nada corre en el hilo de audio.
//! Las selecciones se expresan en fraccion del archivo (0..1).
use std::sync::Arc;
use crate::shared::SessionShared;

#[derive(Clone, Copy, Debug)]
pub enum SampleOp {
    /// Ganancia lineal (1.0 = sin cambio).
    Gain(f32),
    /// Lleva el pico a 0.99.
    Normalize,
    /// Invierte la muestra en el tiempo.
    Reverse,
    /// Silencia el rango (fracciones del archivo).
    Silence(f32, f32),
    FadeIn(f32, f32),
    FadeOut(f32, f32),
    /// El archivo queda solo con el rango. Reajusta loop_bars y pone el offset a 0.
    Trim(f32, f32),
}

fn range_frames(lo: f32, hi: f32, frames: usize) -> (usize, usize) {
    let a = (lo.clamp(0.0, 1.0) * frames as f32) as usize;
    let b = (hi.clamp(0.0, 1.0) * frames as f32) as usize;
    (a.min(b), b.max(a))
}

pub fn apply(shared: &Arc<SessionShared>, t: usize, s: usize, op: SampleOp) -> Result<String, String> {
    let sd = shared.samples.load(t, s).ok_or("El clip no tiene audio cargado")?;
    let ch = sd.channels.max(1) as usize;
    let frames = sd.data.len() / ch;
    if frames == 0 { return Err("Muestra vacia".into()); }
    let mut data = sd.data.clone();
    let mut trimmed = false;
    match op {
        SampleOp::Gain(g) => { for v in data.iter_mut() { *v *= g; } }
        SampleOp::Normalize => {
            let mut peak = 0.0f32;
            for v in data.iter() { let a = v.abs(); if a > peak { peak = a; } }
            if peak < 1e-6 { return Err("La muestra es silencio".into()); }
            let g = 0.99 / peak;
            for v in data.iter_mut() { *v *= g; }
        }
        SampleOp::Reverse => {
            for f in 0..frames / 2 {
                let a = f * ch;
                let b = (frames - 1 - f) * ch;
                for c in 0..ch { data.swap(a + c, b + c); }
            }
        }
        SampleOp::Silence(lo, hi) => {
            let (a, b) = range_frames(lo, hi, frames);
            if b <= a { return Err("Seleccion vacia".into()); }
            for f in a..b { for c in 0..ch { data[f * ch + c] = 0.0; } }
        }
        SampleOp::FadeIn(lo, hi) => {
            let (a, b) = range_frames(lo, hi, frames);
            if b <= a { return Err("Seleccion vacia".into()); }
            let n = (b - a) as f32;
            for f in a..b {
                let g = (f - a) as f32 / n;
                for c in 0..ch { data[f * ch + c] *= g; }
            }
        }
        SampleOp::FadeOut(lo, hi) => {
            let (a, b) = range_frames(lo, hi, frames);
            if b <= a { return Err("Seleccion vacia".into()); }
            let n = (b - a) as f32;
            for f in a..b {
                let g = 1.0 - (f - a) as f32 / n;
                for c in 0..ch { data[f * ch + c] *= g; }
            }
        }
        SampleOp::Trim(lo, hi) => {
            let (a, b) = range_frames(lo, hi, frames);
            if b - a < ch * 16 { return Err("Seleccion demasiado corta".into()); }
            data = data[a * ch..b * ch].to_vec();
            trimmed = true;
        }
    }
    let mut peak = 0.0f32;
    for v in data.iter() { let a = v.abs(); if a > peak { peak = a; } }

    let cur = {
        let st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        st.tracks.get(t).and_then(|tr| tr.clips.get(s)).and_then(|c| c.as_ref())
            .and_then(|c| c.audio_path.clone())
    };
    let src = cur.ok_or("El clip no tiene archivo de audio")?;
    let src_path = std::path::Path::new(&src);
    let stem = src_path.file_stem().and_then(|x| x.to_str()).unwrap_or("muestra");
    let parent = src_path.parent().unwrap_or(std::path::Path::new("/tmp"));
    let mut dest = parent.join(format!("{}_edit.wav", stem));
    let mut n = 1;
    while dest.exists() {
        dest = parent.join(format!("{}_edit{}.wav", stem, n));
        n += 1;
    }
    let spec = hound::WavSpec {
        channels: ch as u16,
        sample_rate: sd.sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(&dest, spec).map_err(|e| e.to_string())?;
    for v in &data { w.write_sample(*v).map_err(|e| e.to_string())?; }
    w.finalize().map_err(|e| e.to_string())?;
    let path = dest.to_string_lossy().into_owned();

    // Recortar cambia la duracion: loop_bars se reajusta con la misma regla que la
    // grabacion (multiplo del ciclo de Groove si esta activo) y el offset vuelve a 0.
    if trimmed {
        let secs = (data.len() / ch) as f64 / sd.sample_rate.max(1) as f64;
        let (tempo, g_bars) = {
            let st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            let tempo = st.tempo.max(20.0);
            let g = if shared.caja_sched.enabled.load(std::sync::atomic::Ordering::Relaxed) {
                shared.caja_sched.schedule.lock()
                    .map(|s| ((s.loop_ticks as u64 / lanzadera_core::TICKS_PER_BAR).max(1)) as f64)
                    .unwrap_or(1.0)
            } else { 1.0 };
            (tempo, g)
        };
        let raw = secs * tempo / 240.0;
        let bars = (((raw / g_bars).round().max(1.0) * g_bars) as i64).clamp(1, 255) as u8;
        shared.edit(|st| {
            if let Some(Some(c)) = st.tracks.get_mut(t).and_then(|tr| tr.clips.get_mut(s)) {
                c.loop_bars = bars;
                c.loop_start_sec = 0.0;
            }
        });
    }
    shared.request_load(t, s, path.clone());
    Ok(format!("{} · pico {:.2}", path, peak))
}
