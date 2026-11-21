import re, shutil, datetime
p = "engine/src/audio_io.rs"
shutil.copy(p, p + ".bak_rc_" + datetime.datetime.now().strftime("%Y%m%d_%H%M%S"))
s = open(p, encoding="utf-8").read()

# 1) 48k (la cura de la saturacion)
if "tasa del grafo" not in s:
    old = "    let sample_rate = supported.sample_rate().0;"
    new = '''    let sample_rate = {
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
    s = s.replace(old, new, 1); print("✅ 48k re-aplicado")
else: print("ℹ 48k presente")

# 2) WINEPREFIX fuera (yabridge auto-detecta prefijo por plugin)
if "quitando WINEPREFIX" not in s:
    a = "    crate::clap_gui::install_alarm_handler();\n"
    assert a in s, "no encontré install_alarm_handler"
    s = s.replace(a, a + '''    if std::env::var_os("LANZADERA_KEEP_WINEPREFIX").is_none() {
        if let Some(v) = std::env::var_os("WINEPREFIX") {
            println!("[lanzadera] quitando WINEPREFIX={:?} (yabridge auto-detecta por plugin)", v);
            std::env::remove_var("WINEPREFIX");
        }
    }
''', 1); print("✅ WINEPREFIX re-aplicado")
else: print("ℹ WINEPREFIX presente")

# 3) Toggle de captura
if "LANZADERA_NO_AUDIO_IN" not in s:
    pat = re.compile(r"let audio_in = match crate::audio_input::open\(&host, sample_rate as usize \* 8\) \{.*?\n    \};", re.DOTALL)
    s = pat.sub('''let audio_in = if std::env::var_os("LANZADERA_NO_AUDIO_IN").is_some() {
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
    };''', s, count=1); print("✅ toggle captura re-aplicado")
else: print("ℹ toggle captura presente")

# 4) Buffer: volver a Default (config probada limpia). Fixed opcional via env.
old_buf = ''' let probed_block = probe_device_block_size(&device, channels as u16, sample_rate);
 let (actual_block, buf_size) = match probed_block {
     Some(b) => (b, cpal::BufferSize::Fixed(b)),
     None    => (PLUGIN_BLOCK_SIZE, cpal::BufferSize::Default),
 };'''
new_buf = ''' // Config probada limpia (ERR=0 en pw-top): BufferSize::Default; el quantum lo
 // fija PipeWire. La saturacion venia del remuestreo 44.1->48k, no del buffer.
 // Los buffers VST3 se crean a MAX_BLOCK para que el render nunca lea fuera.
 // Prueba Fixed con: LANZADERA_BUFFER_FIXED=512
 let (actual_block, buf_size) = match std::env::var("LANZADERA_BUFFER_FIXED")
     .ok().and_then(|v| v.parse::<u32>().ok()).filter(|&b| b >= 64) {
     Some(b) => (b, cpal::BufferSize::Fixed(b)),
     None    => (MAX_BLOCK as u32, cpal::BufferSize::Default),
 };'''
if old_buf in s:
    s = s.replace(old_buf, new_buf, 1); print("✅ buffer -> Default (+override LANZADERA_BUFFER_FIXED)")
else: print("⚠ bloque de buffer no coincidía; revisa a mano")

# probe queda sin usar: silenciar warning
if "#[allow(dead_code)]\nfn probe_device_block_size" not in s:
    s = s.replace("fn probe_device_block_size(", "#[allow(dead_code)]\nfn probe_device_block_size(", 1)
open(p, "w", encoding="utf-8").write(s)
print("✅ audio_io.rs reconciliado: fixes nuestros + watchdog de Claude")
