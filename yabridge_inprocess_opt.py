import re, shutil, datetime
ts = datetime.datetime.now().strftime("%Y%m%d_%H%M%S")

# 1) vst3_slot.rs: que la variable pueda FORZAR in-process (hoy yabridge siempre aísla)
p = "engine/src/vst3_slot.rs"
shutil.copy(p, p + ".bak_sel2_" + ts)
s = open(p, encoding="utf-8").read()
pat = re.compile(r'let isolate = is_yabridge \|\| std::env::var_os\("LANZADERA_VST3_ISOLATE"\)\.is_some\(\);')
new = '''let mode = std::env::var("LANZADERA_VST3_ISOLATE").unwrap_or_default();
    let isolate = match mode.as_str() {
        "1" | "all" | "yes" => true,
        "0" | "none" | "no" => false,   // in-process: sin retardo IPC y con GUI X11
        _ => is_yabridge,               // auto (default): solo yabridge aislado
    };'''
if pat.search(s):
    s = pat.sub(new, s, count=1)
    open(p, "w", encoding="utf-8").write(s)
    print("✅ vst3_slot: aislamiento selectivo restaurado")
elif 'let mode = std::env::var("LANZADERA_VST3_ISOLATE")' in s:
    print("ℹ vst3_slot: ya era selectivo")
else:
    print("⚠ vst3_slot: no encontré la línea de isolate; revisa a mano")

# 2) audio_io.rs: garantizar WINEPREFIX fuera (imprescindible para yabridge in-process)
p = "engine/src/audio_io.rs"
s = open(p, encoding="utf-8").read()
if "quitando WINEPREFIX" not in s:
    shutil.copy(p, p + ".bak_wp2_" + ts)
    a = "    crate::clap_gui::install_alarm_handler();\n"
    ins = a + '''    if std::env::var_os("LANZADERA_KEEP_WINEPREFIX").is_none() {
        if let Some(v) = std::env::var_os("WINEPREFIX") {
            println!("[lanzadera] quitando WINEPREFIX={:?} (yabridge auto-detecta por plugin)", v);
            std::env::remove_var("WINEPREFIX");
        }
    }
'''
    if a in s:
        s = s.replace(a, ins, 1)
        open(p, "w", encoding="utf-8").write(s)
        print("✅ audio_io: WINEPREFIX removal re-aplicado")
    else:
        print("⚠ audio_io: sin ancla install_alarm_handler; añade el bloque WINEPREFIX a mano")
else:
    print("ℹ audio_io: WINEPREFIX presente")
