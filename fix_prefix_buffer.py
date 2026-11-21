import re, shutil, datetime
p = "engine/src/audio_io.rs"
shutil.copy(p, p + ".bak_pb_" + datetime.datetime.now().strftime("%Y%m%d_%H%M%S"))
s = open(p, encoding="utf-8").read()

# 1) WINEPREFIX fuera del entorno, siempre (cubre CLAP y VST3 yabridge).
anchor = "pub fn start(shared: Arc<SessionShared>) -> Result<AudioHandle, String> {\n"
if anchor not in s:
    raise SystemExit("❌ no encontré pub fn start")
inject = '''    // yabridge deduce el prefijo wine de la ruta de cada plugin. Un WINEPREFIX
    // heredado del shell lo fuerza a uno equivocado (plugin en .wine-arturia pero
    // wine en .wine) -> IComponent::initialize falla o crashea. Lo quitamos para
    // que funcione igual desde terminal que desde el menú.
    // Escape: LANZADERA_KEEP_WINEPREFIX=1 conserva la variable.
    if std::env::var_os("LANZADERA_KEEP_WINEPREFIX").is_none() {
        if let Some(v) = std::env::var_os("WINEPREFIX") {
            println!("[lanzadera] quitando WINEPREFIX={:?} (yabridge auto-detecta por plugin)", v);
            std::env::remove_var("WINEPREFIX");
        }
    }
'''
if "quitando WINEPREFIX" not in s:
    s = s.replace(anchor, anchor + inject, 1)

# 2) Buffer de salida explícito y acotado al rango del dispositivo.
old_cfg = '''    let cfg = cpal::StreamConfig {
        channels: channels as u16,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size: cpal::BufferSize::Default,
    };'''
new_cfg = '''    // El Default de ALSA suele ser grande y añade decenas de ms entre pulsar
    // una tecla y oírla. LANZADERA_BUFFER=256 (frames); 0 = Default del driver.
    let want = std::env::var("LANZADERA_BUFFER").ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(256);
    let buffer_size = if want == 0 {
        cpal::BufferSize::Default
    } else {
        match supported.buffer_size() {
            cpal::SupportedBufferSize::Range { min, max } => {
                cpal::BufferSize::Fixed(want.clamp(*min, *max))
            }
            cpal::SupportedBufferSize::Unknown => cpal::BufferSize::Fixed(want),
        }
    };
    if let cpal::BufferSize::Fixed(n) = buffer_size {
        println!("[lanzadera] buffer: {} frames (~{:.1} ms)", n, n as f64 * 1000.0 / sample_rate as f64);
    }
    let cfg = cpal::StreamConfig {
        channels: channels as u16,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size,
    };'''
if old_cfg not in s:
    raise SystemExit("❌ no encontré el bloque StreamConfig (¿ya parcheado?)")
s = s.replace(old_cfg, new_cfg, 1)
open(p, "w", encoding="utf-8").write(s)
print("✅ WINEPREFIX permanente + buffer configurable (LANZADERA_BUFFER)")
