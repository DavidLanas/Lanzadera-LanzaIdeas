import shutil, datetime
p = "engine/src/audio_io.rs"
shutil.copy(p, p + ".bak_cfg_" + datetime.datetime.now().strftime("%Y%m%d_%H%M%S"))
s = open(p, encoding="utf-8").read()
old = "    let is_jack = host.id() == cpal::HostId::Jack;"
new = """    // HostId::Jack solo existe con la feature "jack" de cpal.
    #[cfg(feature = "jack")]
    let is_jack = host.id() == cpal::HostId::Jack;
    #[cfg(not(feature = "jack"))]
    let is_jack = false;"""
if old not in s:
    raise SystemExit("ℹ ya estaba corregido (o no se encontró la línea)")
s = s.replace(old, new, 1)
open(p, "w", encoding="utf-8").write(s)
print("✅ is_jack condicionado a la feature jack")
