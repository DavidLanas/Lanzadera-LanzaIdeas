//! Loader thread — decodifica WAV/FLAC/OGG con symphonia, remuestea al rate
//! del stream y pre-calcula picos para la waveform. Publica en SampleStore
//! (ArcSwapOption). Nunca corre en el hilo de audio.
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::shared::{LoadStatus, SampleData, SessionShared};

pub fn spawn(shared: Arc<SessionShared>, target_rate: u32) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new().name("lanzadera-loader".into()).spawn(move || {
        loop {
            if shared.loader_stop.load(Ordering::Relaxed) { break; }
            drain_rec_slot(&shared);

            let req = { shared.load_queue.lock().unwrap().pop_front() };
            let Some(req) = req else {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            };
            // .mid/.midi -> notas de clip. Audio -> SampleStore.
            // Ambas rutas corren aqui, fuera del callback de audio.
            if crate::midi_file::is_midi_file(&req.path) {
                match crate::midi_file::install_midi(&shared, req.track, req.scene, &req.path) {
                    Ok(n) => {
                        println!("[midi] {} notas -> pista {} escena {}",
                                 n, req.track + 1, req.scene + 1);
                        *shared.status.lock().unwrap() = LoadStatus::Ready;
                    }
                    Err(e) => {
                        *shared.status.lock().unwrap() =
                            LoadStatus::Error(format!("{}: {}", req.path, e));
                    }
                }
                continue;
            }
            match decode_file(&req.path, target_rate) {
                Ok(sd) => {
                    shared.samples.store(req.track, req.scene, Arc::new(sd));
                    *shared.status.lock().unwrap() = LoadStatus::Ready;
                }
                Err(e) => {
                    *shared.status.lock().unwrap() = LoadStatus::Error(format!("{}: {}", req.path, e));
                }
            }
        }
    }).expect("no se pudo lanzar el loader")
}

fn decode_file(path: &str, target_rate: u32) -> Result<SampleData, String> {
    let src = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mss = MediaSourceStream::new(Box::new(src), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = std::path::Path::new(path).extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| e.to_string())?;
    let mut format = probed.format;
    let track = format.tracks().iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or("sin pista de audio")?;
    let track_id = track.id;
    let params = track.codec_params.clone();
    let src_rate = params.sample_rate.unwrap_or(target_rate);
    let channels = params.channels.map(|c| c.count()).unwrap_or(2).max(1);
    let mut decoder = symphonia::default::get_codecs()
        .make(&params, &DecoderOptions::default()).map_err(|e| e.to_string())?;

    let mut interleaved: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() { Ok(p) => p, Err(_) => break };
        if packet.track_id() != track_id { continue; }
        let decoded = match decoder.decode(&packet) { Ok(d) => d, Err(_) => continue };
        let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
        buf.copy_interleaved_ref(decoded);
        interleaved.extend_from_slice(buf.samples());
    }
    if interleaved.is_empty() { return Err("archivo sin muestras".into()); }

    let data = if src_rate != target_rate {
        resample_linear(&interleaved, channels, src_rate, target_rate)
    } else { interleaved };
    let peaks = compute_peaks(&data, channels, 256);
    Ok(SampleData { sample_rate: target_rate, channels: channels as u8, data, peaks })
}

fn resample_linear(data: &[f32], ch: usize, src: u32, dst: u32) -> Vec<f32> {
    let in_frames = data.len() / ch;
    if in_frames == 0 { return Vec::new(); }
    let out_frames = ((in_frames as f64) * (dst as f64 / src as f64)).round() as usize;
    let ratio = src as f64 / dst as f64;
    let mut out = Vec::with_capacity(out_frames * ch);
    for i in 0..out_frames {
        let p = i as f64 * ratio;
        let i0 = (p as usize).min(in_frames - 1);
        let i1 = (i0 + 1).min(in_frames - 1);
        let fr = (p - i0 as f64) as f32;
        for c in 0..ch {
            let a = data[i0 * ch + c];
            let b = data[i1 * ch + c];
            out.push(a + (b - a) * fr);
        }
    }
    out
}

fn compute_peaks(data: &[f32], ch: usize, buckets: usize) -> Vec<f32> {
    let frames = data.len() / ch.max(1);
    let mut out = vec![0.0f32; buckets];
    if frames == 0 { return out; }
    for b in 0..buckets {
        let start = b * frames / buckets;
        let end = (((b + 1) * frames / buckets).max(start + 1)).min(frames);
        let mut m = 0.0f32;
        for f in start..end { for c in 0..ch { m = m.max(data[f * ch + c].abs()); } }
        out[b] = m;
    }
    out
}
fn drain_rec_slot(shared: &SessionShared) {
    let taken = {
        let mut slot = crate::audio_io::REC_SLOT.lock().unwrap_or_else(|e| e.into_inner());
        slot.take()
    };

    let Some((t, s, clip)) = taken else { return };

    let denom = (clip.sample_rate as usize * clip.channels as usize).max(1);
    let secs = clip.samples.len() / denom;
    let wav_path = format!("/tmp/lanzadera_rec_{}_{}.wav", t, s);

    let spec = hound::WavSpec {
        channels: clip.channels,
        sample_rate: clip.sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };

    match hound::WavWriter::create(&wav_path, spec) {
        Ok(mut w) => {
            let mut ok = true;
            for &sm in &clip.samples {
                if w.write_sample(sm).is_err() {
                    ok = false;
                    break;
                }
            }

            if ok && w.finalize().is_ok() {
                println!(
                    "[record] WAV escrito (fuera de RT): {} ({} s)",
                    wav_path, secs
                );

                // Esto es lo que hace falta para que la GUI sepa que el clip
                // tiene audio y pueda dibujar la waveform.
                // El clip debe durar lo que dura el audio grabado. Si se
                // queda en los 2 compases por defecto, la reproduccion
                // loopea solo el principio (eff = min(loop, audio)).
                // Redondeo al compás superior para que quepa todo y el loop
                // quede alineado. Concepto de hybrid track de MAGDA (GPL-3).
                let ch = clip.channels.max(1) as usize;
                let frames = clip.samples.len() / ch;
                let audio_secs = frames as f64 / clip.sample_rate.max(1) as f64;
                let mut bars_used = 0u8;
                // Si Groove está activo, el clip grabado dura un múltiplo de su ciclo
                // (p. ej. 2 compases): así el loop vuelve a caer en el 1.1 de la
                // batería en cada vuelta. Un clip de 5 u 9 compases deriva.
                let g_bars: f64 = if shared.caja_sched.enabled.load(std::sync::atomic::Ordering::Relaxed) {
                    shared.caja_sched.schedule.lock()
                        .map(|s| ((s.loop_ticks as u64 / lanzadera_core::TICKS_PER_BAR).max(1)) as f64)
                        .unwrap_or(1.0)
                } else { 1.0 };
                shared.edit(|st| {
                    let tempo = st.tempo.max(20.0);
                    // 240 = 60 s/min * 4 beats/compás (4/4).
                    let raw = audio_secs * tempo / 240.0;
                    let bars = (((raw / g_bars).round().max(1.0) * g_bars) as i64)
                        .clamp(1, 255) as u8;
                    bars_used = bars;
                    if let Some(tr) = st.tracks.get_mut(t) {
                        if let Some(slot) = tr.clips.get_mut(s) {
                            if let Some(c) = slot {
                                c.audio_path = Some(wav_path.clone());
                                c.loop_bars = bars;
                            }
                        }
                    }
                });
                println!(
                    "[record] clip ajustado: {} compases para {:.1} s de audio",
                    bars_used, audio_secs
                );

                // Reusa el camino que ya funciona al insertar un .wav a mano:
                // load_queue -> decode_file -> compute_peaks -> samples.store.
                if let Ok(mut q) = shared.load_queue.lock() {
                    q.push_back(crate::shared::LoadRequest {
                        track: t,
                        scene: s,
                        path: wav_path,
                    });
                }
            } else {
                eprintln!("[record] error al escribir WAV: {}", wav_path);
            }
        }
        Err(e) => eprintln!("[record] error al crear WAV: {}", e),
    }

    // Devolver el buffer al pool para que la siguiente grabacion no asigne
    // memoria en el callback RT. Estamos fuera de RT (hilo del loader).
    crate::audio_record::recycle_buffer(clip.samples);
}
