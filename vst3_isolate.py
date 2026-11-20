import os, re, shutil, datetime
if not os.path.isdir("engine/src"):
    print("❌ Ejecuta desde la raíz del repo"); exit(1)
p = "engine/src/vst3_slot.rs"
shutil.copy(p, p + ".bak_iso_" + datetime.datetime.now().strftime("%Y%m%d_%H%M%S"))
s = open(p, encoding="utf-8").read()

# Asegurar helper err_chain (por si no se aplicó Batch 1)
if "fn err_chain(" not in s:
    helper = '''
fn err_chain(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut src = e.source();
    while let Some(c) = src { s.push_str(" <- "); s.push_str(&c.to_string()); src = c.source(); }
    s
}

'''
    s = s.replace("/// Carga y activa un plugin VST3.", helper + "/// Carga y activa un plugin VST3.", 1)

pat = re.compile(
    r"    let mut plugin = simple::load_plugin_with_settings\(&bundle, sample_rate as f64, max_frames as usize\)\s*"
    r"\.map_err\(\|e\| format!\(\"load_plugin\(\{\}\): \{\}\", bundle\.display\(\), (?:err_chain\(&e\)|e)\)\)\?;",
    re.DOTALL)
new = '''    // Modo seguro opcional (como LYNX/Bitwig): hosting en proceso separado.
    // Si el plugin o el proxy de yabridge hace SIGSEGV, muere el helper y
    // Lanzadera sobrevive. Activar con: LANZADERA_VST3_ISOLATE=1
    let isolate = std::env::var_os("LANZADERA_VST3_ISOLATE").is_some();
    let mut plugin = if isolate {
        println!("[vst3] cargando en proceso aislado (LANZADERA_VST3_ISOLATE=1)");
        let mut host = vst3_host::Vst3Host::builder()
            .sample_rate(sample_rate as f64)
            .block_size(max_frames as usize)
            .input_channels(2)
            .output_channels(2)
            .with_process_isolation(true)
            .build()
            .map_err(|e| format!("host builder: {}", err_chain(&e)))?;
        host.load_plugin(&bundle)
            .map_err(|e| format!("load_plugin aislado({}): {}", bundle.display(), err_chain(&e)))?
    } else {
        simple::load_plugin_with_settings(&bundle, sample_rate as f64, max_frames as usize)
            .map_err(|e| format!("load_plugin({}): {}", bundle.display(), err_chain(&e)))?
    };'''
if not pat.search(s):
    raise SystemExit("❌ no encontré la línea de load_plugin_with_settings (¿ya parcheado?)")
s = pat.sub(new, s, count=1)
open(p, "w", encoding="utf-8").write(s)
print("✅ vst3_slot.rs: modo aislado opt-in añadido")
