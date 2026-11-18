//! Cargador de samples en background. Usa symphonia para decodificar
//! WAV/FLAC/OGG/MP3 sin bloquear el audio.
//!
//! Estructura soportada (en este orden):
//! 1. Subcarpetas numeradas `00/`..`24/` → cada carpeta es el banco de un pad
//!    (override manual: no se clasifica nada).
//! 2. Carpeta cualquiera → se clasifica por NOMBRE con la tabla de palabras
//!    clave de caja-core (`KeywordTable::builtin`). Cada muestra va al pad
//!    que le corresponde (kick al kick, snare al snare…). Las que no cumplen
//!    ningún nombre quedan en la bandeja "Sin clasificar" para que el usuario
//!    decida a qué pad van (`GrooveLibrary::assign_to_pad`).
//!
//! El tope de muestras por pad (`bank_cap`, 8..=128) es el control de RAM.
//!
//! Ruta: variable de entorno `CAJA_SAMPLES`, si no la de por defecto.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use caja_core::kit::NUM_PADS;
use caja_core::library::{KeywordTable, Library, MAX_BANK, MIN_BANK};
use caja_core::pieces::{PAD_ORDER, PIECES};

use crate::caja_drums::library_state;
use crate::caja_drums::sample::Sample;
use crate::caja_drums::store::CajaStore;

/// Cola lock-free de requests de carga (audio → loader).
pub struct RequestQueue {
    buf: [std::sync::atomic::AtomicU32; 256],
    head: std::sync::atomic::AtomicU32,
    tail: std::sync::atomic::AtomicU32,
}

impl RequestQueue {
    pub fn new() -> Self {
        RequestQueue {
            buf: std::array::from_fn(|_| std::sync::atomic::AtomicU32::new(0)),
            head: std::sync::atomic::AtomicU32::new(0),
            tail: std::sync::atomic::AtomicU32::new(0),
        }
    }

    pub fn push(&self, val: u32) -> bool {
        let h = self.head.load(Ordering::Relaxed);
        let t = self.tail.load(Ordering::Acquire);
        if h.wrapping_sub(t) >= 256 { return false; }
        self.buf[h as usize % 256].store(val, Ordering::Relaxed);
        self.head.store(h.wrapping_add(1), Ordering::Release);
        true
    }

    #[allow(dead_code)]
    pub fn pop(&self) -> Option<u32> {
        let t = self.tail.load(Ordering::Relaxed);
        let h = self.head.load(Ordering::Acquire);
        if t == h { return None; }
        let val = self.buf[t as usize % 256].load(Ordering::Relaxed);
        self.tail.store(t.wrapping_add(1), Ordering::Release);
        Some(val)
    }
}

const DEFAULT_SAMPLES_PATH: &str = "/home/tarro/Música/loops nuevos";

fn samples_base() -> PathBuf {
    std::env::var_os("CAJA_SAMPLES")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SAMPLES_PATH))
}

fn is_audio(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|s| s.to_str()).map(|s| s.to_ascii_lowercase()).as_deref(),
        Some("wav" | "flac" | "ogg" | "mp3" | "aiff" | "aif")
    )
}

pub struct CajaLoader {
    #[allow(dead_code)]
    store: Arc<CajaStore>,
    #[allow(dead_code)]
    sample_rate: u32,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl CajaLoader {
    pub fn spawn(store: Arc<CajaStore>, sample_rate: u32) -> Self {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let s = Self { store: store.clone(), sample_rate, stop: stop.clone() };

        std::thread::Builder::new()
            .name("caja-loader".into())
            .spawn(move || Self::run(store, stop))
            .ok();
        s
    }

    fn run(store: Arc<CajaStore>, stop: Arc<std::sync::atomic::AtomicBool>) {
        let table = KeywordTable::builtin();
        let shared = library_state::shared();

        loop {
            if stop.load(Ordering::Relaxed) { return; }

            // ── Configuración viva (la UI puede cambiarla en caliente) ──
            let (root_req, assign, cap, max_len) = {
                let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
                g.dirty = false;
                (g.root_request.clone(), g.assign.clone(),
                 g.bank_cap.clamp(MIN_BANK, MAX_BANK), g.max_len_ms)
            };

            // Raíz: petición del usuario → env CAJA_SAMPLES → default.
            let base = if !root_req.is_empty() {
                PathBuf::from(&root_req)
            } else {
                samples_base()
            };

            if !base.exists() {
                println!("[groove-loader] la carpeta no existe: {:?}", base);
                {
                    let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
                    g.root = base.to_string_lossy().to_string();
                    g.lib = Library::empty();
                    g.loaded = [0; NUM_PADS];
                    g.files.clear();
                }
                Self::wait_dirty(&store, &shared, &stop);
                continue;
            }

            let root = base.to_string_lossy().to_string();

            // ── Modo 1: carpetas numeradas 00/..24/ (override manual) ──
            let has_pad_dirs = (0..NUM_PADS).any(|p| base.join(format!("{:02}", p)).is_dir());
            if has_pad_dirs {
                let per_pad = Self::load_numbered(&store, &stop, &base, cap);
                let total: usize = per_pad.iter().sum();
                {
                    let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
                    g.numbered = true;
                    g.root = root.clone();
                    g.loaded = per_pad;
                    g.skipped_long = 0;
                }
                println!("[groove-loader] carpetas numeradas en {:?}: {} muestras", base, total);
                Self::wait_dirty(&store, &shared, &stop);
                continue;
            }

            // ── Modo 2: clasificación por nombre ──
            let mut files = Vec::new();
            Self::collect_rel(&base, &base, &mut files, 0);
            files.sort();
            files.dedup();
            let found = files.len();

            // Filtro de duración: excluye loops antes de construir el banco,
            // así los 32 slots de cada pad se llenan con one-shots reales.
            let mut skipped_long = 0usize;
            if max_len > 0 {
                files.retain(|rel| {
                    match probe_duration_ms(&base.join(rel)) {
                        Some(ms) if ms > max_len => { skipped_long += 1; false }
                        _ => true,   // sin dato de duración → se acepta
                    }
                });
            }

            let lib = Library::build(&root, &files, &table, &assign);
            let classified = lib.total_classified();
            let unclass = lib.unclassified.len();

            let mut loaded = 0usize;
            let mut per_pad = [0usize; NUM_PADS];
            for pad in 0..NUM_PADS {
                if stop.load(Ordering::Relaxed) { return; }
                for (slot, rel) in lib.bank(pad, cap).iter().enumerate() {
                    if stop.load(Ordering::Relaxed) { return; }
                    if let Some(s) = decode_to_sample(&base.join(rel)) {
                        store.store_sample(pad, slot, Arc::new(s));
                        loaded += 1;
                        per_pad[pad] += 1;
                    }
                }
            }

            {
                let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
                g.lib = lib;
                g.loaded = per_pad;
                g.files = files;
                g.root = root;
                g.numbered = false;
                g.skipped_long = skipped_long;
            }

            println!("[groove-loader] {:?}", base);
            println!(
                "  {} archivos · {} excluidos por >{} ms · {} clasificadas · {} SIN CLASIFICAR · {} cargadas (cap {}/pad)",
                found, skipped_long, max_len, classified, unclass, loaded, cap
            );
            for (pad, n) in per_pad.iter().enumerate() {
                if *n > 0 {
                    let piece = PAD_ORDER[pad];
                    println!("  pad {:02} {:<16} → {}", pad, PIECES[piece].name, n);
                }
            }

            Self::wait_dirty(&store, &shared, &stop);
            println!("[groove-loader] reconstruyendo biblioteca…");
        }
    }

    /// Espera a que la UI marque `dirty` (o a que se pida parar).
    /// De paso resuelve los pins pendientes: solo este hilo conoce el banco
    /// definitivo tras cada reconstrucción.
    fn wait_dirty(
        store: &Arc<CajaStore>,
        shared: &Arc<std::sync::Mutex<library_state::GrooveLibrary>>,
        stop: &Arc<std::sync::atomic::AtomicBool>,
    ) {
        loop {
            if stop.load(Ordering::Relaxed) { return; }
            std::thread::sleep(Duration::from_millis(250));
            if shared.lock().unwrap_or_else(|e| e.into_inner()).pins_dirty {
                Self::resolve_pins(store, shared);
            }
            let d = shared.lock().unwrap_or_else(|e| e.into_inner()).dirty;
            if d { return; }
        }
    }

    /// Traduce las muestras fijadas (ruta) a `pin_idx` del almacén, que es lo
    /// único que consulta `CajaEngine::trigger`. Marca `pin_missing` cuando el
    /// archivo ya no está en el banco.
    fn resolve_pins(
        store: &Arc<CajaStore>,
        shared: &Arc<std::sync::Mutex<library_state::GrooveLibrary>>,
    ) {
        let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
        if !g.pins_dirty { return; }
        for pad in 0..NUM_PADS {
            let pin = g.pins[pad].clone();
            let idx = match &pin {
                Some((hint, rel)) => {
                    if g.numbered {
                        // Bancos por carpetas numeradas: vale el índice guardado.
                        let n = g.loaded[pad];
                        if n > 0 && *hint < n { Some(*hint) } else { None }
                    } else {
                        g.lib.find_in_bank(pad, g.bank_cap, rel)
                    }
                }
                None => None,
            };
            store.pin_idx[pad].store(idx.map(|i| i as i32).unwrap_or(-1), Ordering::Relaxed);
            store.pin_missing[pad].store(pin.is_some() && idx.is_none(), Ordering::Relaxed);
        }
        g.pins_dirty = false;
    }

    /// Modo de carpetas numeradas: cada `NN/` es el banco del pad NN.
    fn load_numbered(
        store: &Arc<CajaStore>,
        stop: &Arc<std::sync::atomic::AtomicBool>,
        base: &Path,
        cap: usize,
    ) -> [usize; NUM_PADS] {
        let mut per_pad = [0usize; NUM_PADS];
        for pad in 0..NUM_PADS {
            if stop.load(Ordering::Relaxed) { break; }
            let pad_dir = base.join(format!("{:02}", pad));
            if !pad_dir.is_dir() { continue; }
            let entries = Self::audio_files(&pad_dir);
            for (idx, path) in entries.iter().enumerate().take(cap) {
                if stop.load(Ordering::Relaxed) { break; }
                if let Some(s) = decode_to_sample(path) {
                    store.store_sample(pad, idx, Arc::new(s));
                    per_pad[pad] += 1;
                }
            }
        }
        per_pad
    }

    /// Recoge rutas relativas al `base` (hasta profundidad 4).
    fn collect_rel(base: &Path, dir: &Path, out: &mut Vec<String>, depth: usize) {
        if depth > 4 { return; }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                Self::collect_rel(base, &p, out, depth + 1);
            } else if p.is_file() && is_audio(&p) {
                if let Ok(rel) = p.strip_prefix(base) {
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }

    fn audio_files(dir: &Path) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(dir).ok()
            .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect())
            .unwrap_or_default();
        v.sort();
        v.retain(|p| p.is_file() && is_audio(p));
        v
    }
}

impl Drop for CajaLoader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn decode_to_sample(path: &Path) -> Option<Sample> {
    let file = std::fs::File::open(path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .ok()?;
    let mut reader = probed.format;
    let track = reader.default_track()?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default()).ok()?;

    let mut left = Vec::new();
    let mut right = Vec::new();
    let channels = track.codec_params.channels.map(|c| c.count()).unwrap_or(1);
    let is_stereo = channels >= 2;

    loop {
        let packet = match reader.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::ResetRequired) => break,
            Err(symphonia::core::errors::Error::IoError(e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(_) => break,
        };
        if packet.track_id() != track_id { continue; }
        let decoded = match decoder.decode(&packet) { Ok(d) => d, Err(_) => continue };
        let spec = *decoded.spec();
        let mut sbuf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        sbuf.copy_interleaved_ref(decoded);
        let buf = sbuf.samples();
        if is_stereo {
            for ch in buf.chunks_exact(2) {
                left.push(ch[0]);
                right.push(ch[1]);
            }
        } else {
            left.extend_from_slice(buf);
        }
    }

    if left.is_empty() { return None; }
    Some(Sample { left, right: if is_stereo { Some(right) } else { None } })
}

/// Duración en ms leyendo solo la cabecera (rápido). None si el formato
/// no expone n_frames/sample_rate (p.ej. algunos MP3/OGG).
fn probe_duration_ms(path: &Path) -> Option<u32> {
    let file = std::fs::File::open(path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .ok()?;
    let track = probed.format.default_track()?;
    let sr = track.codec_params.sample_rate? as f64;
    let n  = track.codec_params.n_frames? as f64;
    if sr <= 0.0 { return None; }
    Some((n / sr * 1000.0).round() as u32)
}
