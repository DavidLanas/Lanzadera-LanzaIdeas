import re, shutil, datetime
p = "engine/src/audio_io.rs"
shutil.copy(p, p + ".bak_ain_" + datetime.datetime.now().strftime("%Y%m%d_%H%M%S"))
s = open(p, encoding="utf-8").read()
pat = re.compile(r"    let audio_in = match crate::audio_input::open\(&host, sample_rate as usize \* 8\) \{.*?\n    \};", re.DOTALL)
if not pat.search(s):
    raise SystemExit("❌ no encontré el bloque audio_in (¿ya parcheado?)")
new = '''    let audio_in = if std::env::var_os("LANZADERA_NO_AUDIO_IN").is_some() {
        println!("[lanzadera] audio input: desactivado (LANZADERA_NO_AUDIO_IN=1)");
        (None, None, 0u16)
    } else {
        match crate::audio_input::open(&host, sample_rate as usize * 8) {
            Ok((stream, cons)) => {
                println!(
                    "[lanzadera] audio input: {} Hz · {} canales",
                    stream.sample_rate, stream.channels
                );
                let ch = stream.channels;
                (Some(stream), Some(cons), ch)
            }
            Err(e) => {
                eprintln!("[lanzadera] audio input: {}", e);
                (None, None, 0u16)
            }
        }
    };'''
s = pat.sub(new, s, count=1)
open(p, "w", encoding="utf-8").write(s)
print("✅ Entrada de audio desactivable con LANZADERA_NO_AUDIO_IN=1")
