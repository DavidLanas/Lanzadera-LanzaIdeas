import re, shutil, datetime, os
ts = datetime.datetime.now().strftime("%Y%m%d_%H%M%S")

# --- A) app/Cargo.toml: exponer feature jack ---
p = "app/Cargo.toml"
shutil.copy(p, p + ".bak_pw_" + ts)
s = open(p, encoding="utf-8").read()
if re.search(r'^\s*jack\s*=', s, re.M):
    print("ℹ app ya tiene feature jack")
else:
    m = re.search(r'([\w-]*engine[\w-]*)/clap', s)
    eng = m.group(1) if m else "lanzadera-engine"
    if "[features]" not in s:
        s += '\n[features]\njack = ["%s/jack"]\n' % eng
    else:
        s = s.replace("[features]", '[features]\njack = ["%s/jack"]' % eng, 1)
    open(p, "w", encoding="utf-8").write(s)
    print(f"✅ app: feature jack -> {eng}/jack")

# --- B) audio_io.rs: en JACK el quantum lo pone el servidor ---
p = "engine/src/audio_io.rs"
shutil.copy(p, p + ".bak_pw_" + ts)
s = open(p, encoding="utf-8").read()

if "let buffer_size = if want == 0 {" in s:
    s = s.replace(
        "    let buffer_size = if want == 0 {",
        "    // En JACK/PipeWire el buffer lo fija el servidor (PIPEWIRE_LATENCY).\n"
        "    let is_jack = host.id() == cpal::HostId::Jack;\n"
        "    let buffer_size = if want == 0 || is_jack {", 1)
    s = s.replace(
        '    if let cpal::BufferSize::Fixed(n) = buffer_size {\n'
        '        println!("[lanzadera] buffer: {} frames (~{:.1} ms)", n, n as f64 * 1000.0 / sample_rate as f64);\n'
        '    }',
        '    if let cpal::BufferSize::Fixed(n) = buffer_size {\n'
        '        println!("[lanzadera] buffer: {} frames (~{:.1} ms)", n, n as f64 * 1000.0 / sample_rate as f64);\n'
        '    }\n'
        '    if is_jack {\n'
        '        println!("[lanzadera] JACK/PipeWire: quantum del servidor (PIPEWIRE_LATENCY)");\n'
        '    }', 1)
    open(p, "w", encoding="utf-8").write(s)
    print("✅ buffer JACK-aware")
elif "buffer_size: cpal::BufferSize::Default," in s:
    print("⚠ no está el parche de buffer previo; aplica fix_prefix_buffer.py primero")
else:
    print("ℹ buffer ya adaptado")

# --- C) mensaje más claro en start() ---
s = open(p, encoding="utf-8").read()
s = s.replace('println!("[lanzadera] audio: JACK");',
              'println!("[lanzadera] audio: JACK (PipeWire si se lanzó con pw-jack)");', 1)
open(p, "w", encoding="utf-8").write(s)
