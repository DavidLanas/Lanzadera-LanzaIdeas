import re, shutil, datetime
p = "engine/src/vst3_slot.rs"
shutil.copy(p, p + ".bak_sel_" + datetime.datetime.now().strftime("%Y%m%d_%H%M%S"))
s = open(p, encoding="utf-8").read()

old = 'let isolate = std::env::var_os("LANZADERA_VST3_ISOLATE").is_some();'
new = '''// Aislamiento SELECTIVO (estrategia LYNX: aislar solo lo problemático).
    //   LANZADERA_VST3_ISOLATE=1|all  -> todo aislado
    //   LANZADERA_VST3_ISOLATE=0|none -> nada aislado
    //   sin variable (auto)           -> solo rutas yabridge (wine)
    // Los VST3 nativos van in-process: su editor X11 funciona ahí. El helper
    // todavía NO implementa CreateGui/CloseGui en Linux (solo macOS).
    let mode = std::env::var("LANZADERA_VST3_ISOLATE").unwrap_or_default();
    let is_yabridge = bundle.to_string_lossy().contains("yabridge");
    let isolate = match mode.as_str() {
        "1" | "all" | "yes" => true,
        "0" | "none" | "no" => false,
        _ => is_yabridge,
    };'''
if old not in s:
    raise SystemExit("❌ no encontré la línea de isolate (¿ya parcheado?)")
s = s.replace(old, new, 1)
s = s.replace('println!("[vst3] cargando en proceso aislado (LANZADERA_VST3_ISOLATE=1)");',
              'println!("[vst3] {} -> proceso aislado", bundle.display());', 1)
open(p, "w", encoding="utf-8").write(s)
print("✅ Aislamiento selectivo: yabridge aislado, nativos in-process")
