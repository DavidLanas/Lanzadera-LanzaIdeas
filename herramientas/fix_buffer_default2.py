import re, shutil, datetime
p = "engine/src/audio_io.rs"
shutil.copy(p, p + ".bak_bd2_" + datetime.datetime.now().strftime("%Y%m%d_%H%M%S"))
s = open(p, encoding="utf-8").read()

print("ESTADO ACTUAL:")
print("  probe/Fixed buffer :", "SÍ" if "probe_device_block_size(&device" in s else "NO")
print("  48k                :", "SÍ" if "tasa del grafo" in s else "NO")
print("  WINEPREFIX         :", "SÍ" if "quitando WINEPREFIX" in s else "NO")
print("  toggle captura     :", "SÍ" if "LANZADERA_NO_AUDIO_IN" in s else "NO")

# 1) Neutralizar CUALQUIER Fixed del probe -> Default (override opcional)
pat = re.compile(
    r"let probed_block = probe_device_block_size\(&device, channels as u16, sample_rate\);\s*"
    r"let \(actual_block, buf_size\) = match probed_block \{.*?\};", re.S)
new = '''let actual_block = MAX_BLOCK as u32;
 let buf_size = match std::env::var("LANZADERA_BUFFER_FIXED").ok()
     .and_then(|v| v.parse::<u32>().ok()).filter(|&b| b >= 64) {
     Some(b) => cpal::BufferSize::Fixed(b),   // solo experimentos
     None    => cpal::BufferSize::Default,    // config probada limpia bajo pipewire-alsa
 };'''
if pat.search(s):
    s = pat.sub(new, s, count=1); print("✅ probe/Fixed -> Default")
else:
    n = len(s)
    s = re.sub(r"buffer_size:\s*cpal::BufferSize::Fixed\([^)]*\)",
               "buffer_size: cpal::BufferSize::Default", s)
    print("✅ Fixed suelto -> Default" if len(s) != n else "ℹ buffer ya en Default")

if "#[allow(dead_code)]\nfn probe_device_block_size" not in s:
    s = s.replace("fn probe_device_block_size(", "#[allow(dead_code)]\nfn probe_device_block_size(", 1)

# 2) Reponer 48k si falta
if "tasa del grafo" not in s:
    old = "    let sample_rate = supported.sample_rate().0;"
    new48 = '''    let sample_rate = {
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
    };'''
    assert old in s, "no encontré sample_rate"
    s = s.replace(old, new48, 1); print("✅ 48k repuesto")

# 3) Reponer WINEPREFIX si falta
if "quitando WINEPREFIX" not in s:
    a1 = "    crate::clap_gui::install_alarm_handler();\n"
    a2 = "pub fn start(shared: Arc<SessionShared>) -> Result<AudioHandle, String> {\n"
    ins = '''    if std::env::var_os("LANZADERA_KEEP_WINEPREFIX").is_none() {
        if let Some(v) = std::env::var_os("WINEPREFIX") {
            println!("[lanzadera] quitando WINEPREFIX={:?} (yabridge auto-detecta por plugin)", v);
            std::env::remove_var("WINEPREFIX");
        }
    }
'''
    if a1 in s: s = s.replace(a1, a1 + ins, 1)
    else: s = s.replace(a2, a2 + ins, 1)
    print("✅ WINEPREFIX repuesto")

# 4) Reponer toggle de captura si falta
if "LANZADERA_NO_AUDIO_IN" not in s:
    pat2 = re.compile(r"let audio_in = match crate::audio_input::open\(&host, sample_rate as usize \* 8\) \{.*?\n    \};", re.S)
    s = pat2.sub('''let audio_in = if std::env::var_os("LANZADERA_NO_AUDIO_IN").is_some() {
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
    print("✅ toggle captura repuesto")

open(p, "w", encoding="utf-8").write(s)
print("\n✅ audio_io.rs normalizado: Default + 48k + WINEPREFIX + toggle")
