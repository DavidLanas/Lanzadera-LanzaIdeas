import shutil, datetime
p = "engine/src/audio_input.rs"
shutil.copy(p, p + ".bak_inrate_" + datetime.datetime.now().strftime("%Y%m%d_%H%M%S"))
s = open(p, encoding="utf-8").read()
old = "    let sample_rate = config.sample_rate().0;"
new = '''    // Alinear la captura a la tasa del grafo (48000), igual que la salida.
    // Si entrada y salida van a tasas distintas, el monitoreo y lo grabado
    // suenan a otra velocidad/tono. Override: LANZADERA_RATE.
    let sample_rate = {
        let default_rate = config.sample_rate().0;
        match std::env::var("LANZADERA_RATE").ok().and_then(|v| v.parse::<u32>().ok()) {
            Some(r) if (8000..=384000).contains(&r) => r,
            _ => {
                let has48 = device.supported_input_configs().ok()
                    .map(|mut it| it.any(|c| c.min_sample_rate().0 <= 48000 && 48000 <= c.max_sample_rate().0))
                    .unwrap_or(false);
                if has48 && default_rate != 48000 {
                    println!("[lanzadera] entrada: usando 48000 Hz (tasa del grafo) en vez de {} Hz", default_rate);
                    48000
                } else {
                    default_rate
                }
            }
        }
    };'''
if old not in s:
    raise SystemExit("❌ no encontré la línea de sample_rate de entrada (¿ya parcheado?)")
s = s.replace(old, new, 1)
open(p, "w", encoding="utf-8").write(s)
print("✅ captura alineada a 48000 (misma regla que la salida)")
