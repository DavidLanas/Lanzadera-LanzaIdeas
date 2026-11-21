import shutil, datetime
p = "engine/src/audio_io.rs"
shutil.copy(p, p + ".bak_rate_" + datetime.datetime.now().strftime("%Y%m%d_%H%M%S"))
s = open(p, encoding="utf-8").read()
old = "    let sample_rate = supported.sample_rate().0;"
new = '''    // Alinear la app a la tasa del grafo PipeWire (48000) para evitar remuestreo
    // por bloque, que bajo pipewire-alsa es una fuente tipica de xruns.
    // Override manual: LANZADERA_RATE=48000 (o la tasa que quieras forzar).
    let sample_rate = {
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
                } else {
                    default_rate
                }
            }
        }
    };'''
if old not in s:
    raise SystemExit("❌ no encontré la línea de sample_rate (¿ya parcheado?)")
s = s.replace(old, new, 1)
open(p, "w", encoding="utf-8").write(s)
print("✅ sample rate alineado al grafo (48000) con override LANZADERA_RATE")
