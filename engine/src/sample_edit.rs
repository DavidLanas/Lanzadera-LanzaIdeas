//! Edicion de la muestra de un clip (lote C3). No destructiva: cada operacion
//! escribe un WAV nuevo junto al original y recarga el clip por el loader.
//! El archivo original nunca se modifica. Nada de esto corre en el hilo de audio.
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
}

pub fn apply(shared: &Arc<SessionShared>, t: usize, s: usize, op: SampleOp) -> Result<String, String> {
    let sd = shared.samples.load(t, s).ok_or("El clip no tiene audio cargado")?;
    let ch = sd.channels.max(1) as usize;
    let frames = sd.data.len() / ch;
    if frames == 0 { return Err("Muestra vacia".into()); }
    let mut data = sd.data.clone();
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
    shared.request_load(t, s, path.clone());
    Ok(format!("{} · pico {:.2}", path, peak))
}
