import re, shutil, datetime
ts = datetime.datetime.now().strftime("%Y%m%d_%H%M%S")

def rd(p): return open(p, encoding="utf-8").read()
def wr(p, s): open(p, "w", encoding="utf-8").write(s)

# ── Cargo.toml: libc para el watchdog ──
p = "engine/Cargo.toml"; s = rd(p)
if 'libc = "0.2"' not in s:
    s = s.replace('[features]', 'libc = "0.2"\n\n[features]', 1)
    wr(p, s); print("✅ Cargo.toml: libc añadido")
else: print("ℹ Cargo.toml: libc ya estaba")

# ── audio_io.rs ──
p = "engine/src/audio_io.rs"; shutil.copy(p, p + ".bak_mg_" + ts); s = rd(p)

# 1) Watchdog: llamar install_alarm_handler al arrancar
if "install_alarm_handler()" not in s:
    s = s.replace("pub fn start(shared: Arc<SessionShared>) -> Result<AudioHandle, String> {",
                  "pub fn start(shared: Arc<SessionShared>) -> Result<AudioHandle, String> {\n    crate::clap_gui::install_alarm_handler();", 1)
    print("✅ audio_io: install_alarm_handler() llamado")
else: print("ℹ audio_io: install_alarm_handler ya llamado")

# 2) Re-aplicar 48k si Claude lo pisó
if "tasa del grafo" not in s and "LANZADERA_RATE" not in s:
    s = s.replace("    let sample_rate = supported.sample_rate().0;",
        '''    let sample_rate = {
        let default_rate = supported.sample_rate().0;
        match std::env::var("LANZADERA_RATE").ok().and_then(|v| v.parse::<u32>().ok()) {
            Some(r) if (8000..=384000).contains(&r) => r,
            _ => {
                let has48 = device.supported_output_configs().ok()
                    .map(|mut it| it.any(|c| c.min_sample_rate().0 <= 48000 && 48000 <= c.max_sample_rate().0))
                    .unwrap_or(false);
                if has48 && default_rate != 48000 {
                    println!("[lanzadera] usando 48000 Hz (tasa del grafo) en vez de {} Hz", default_rate);
                    48000
                } else { default_rate }
            }
        }
    };''', 1)
    print("✅ audio_io: 48k re-aplicado")
else: print("ℹ audio_io: 48k presente")

# 3) Re-aplicar WINEPREFIX si Claude lo pisó
if "quitando WINEPREFIX" not in s:
    s = s.replace("pub fn start(shared: Arc<SessionShared>) -> Result<AudioHandle, String> {",
        '''pub fn start(shared: Arc<SessionShared>) -> Result<AudioHandle, String> {
    if std::env::var_os("LANZADERA_KEEP_WINEPREFIX").is_none() {
        if let Some(v) = std::env::var_os("WINEPREFIX") {
            println!("[lanzadera] quitando WINEPREFIX={:?} (yabridge auto-detecta por plugin)", v);
            std::env::remove_var("WINEPREFIX");
        }
    }''', 1)
    print("✅ audio_io: WINEPREFIX re-aplicado")
else: print("ℹ audio_io: WINEPREFIX presente")

# 4) Re-aplicar toggle de captura si Claude lo pisó
if "LANZADERA_NO_AUDIO_IN" not in s:
    pat = re.compile(r"    let audio_in = match crate::audio_input::open\(&host, sample_rate as usize \* 8\) \{.*?\n    \};", re.DOTALL)
    s = pat.sub('''    let audio_in = if std::env::var_os("LANZADERA_NO_AUDIO_IN").is_some() {
        println!("[lanzadera] audio input: desactivado (LANZADERA_NO_AUDIO_IN=1)");
        (None, None, 0u16)
    } else {
        match crate::audio_input::open(&host, sample_rate as usize * 8) {
            Ok((stream, cons)) => {
                println!("[lanzadera] audio input: {} Hz · {} canales", stream.sample_rate, stream.channels);
                let ch = stream.channels;
                (Some(stream), Some(cons), ch)
            }
            Err(e) => { eprintln!("[lanzadera] audio input: {}", e); (None, None, 0u16) }
        }
    };''', s, count=1)
    print("✅ audio_io: toggle captura re-aplicado")
else: print("ℹ audio_io: toggle captura presente")

# 5) NEUTRALIZAR el buffer Fixed(512)+buffers VST3 a 512 (peligroso bajo PipeWire).
#    Volvemos a Default + buffers MAX_BLOCK: es la config que ya sonó limpia.
#    La latencia se controla con el quantum de PipeWire, no con cpal Fixed.
if "probe_device_block_size(&device" in s:
    s = s.replace(''' let probed_block = probe_device_block_size(&device, channels as u16, sample_rate);
 let (actual_block, buf_size) = match probed_block {
     Some(b) => (b, cpal::BufferSize::Fixed(b)),
     None    => (PLUGIN_BLOCK_SIZE, cpal::BufferSize::Default),
 };''',
''' // Bajo pipewire-alsa el periodo real puede exceder el Fixed pedido (el quantum
 // lo fija el grafo). Los buffers VST3 deben cubrir MAX_BLOCK para que el render
 // nunca lea fuera de rango. Default + vst3_slot::process(numSamples por bloque)
 // es la combinacion que ya dio ERR=0 en pw-top.
 let _probed = probe_device_block_size(&device, channels as u16, sample_rate);
 let actual_block = MAX_BLOCK as u32;
 let buf_size = cpal::BufferSize::Default;''', 1)
    print("✅ audio_io: buffer Fixed(512) neutralizado -> Default + MAX_BLOCK")
else: print("ℹ audio_io: sin bloque probe (nada que neutralizar)")
wr(p, s)

print("\n🔎 Estado de vst3_slot.rs (detector yabridge de Claude):")
v = rd("engine/src/vst3_slot.rs")
print("   is_yabridge_bundle:", "SÍ" if "fn is_yabridge_bundle" in v else "NO")
print("   auto-aislamiento  :", "SÍ" if "is_yabridge ||" in v else "NO")
print("   process con in_l/in_r:", "SÍ" if "in_l: &[f32]" in v else "NO")
print("🔎 Estado de clap_gui.rs (watchdog de Claude):")
g = rd("engine/src/clap_gui.rs")
print("   crash_guard       :", "SÍ" if "mod crash_guard" in g else "NO")
print("   install_alarm     :", "SÍ" if "pub fn install_alarm_handler" in g else "NO")
