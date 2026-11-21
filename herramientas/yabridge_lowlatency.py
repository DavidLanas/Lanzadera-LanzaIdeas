import re, shutil, datetime
ts = datetime.datetime.now().strftime("%Y%m%d_%H%M%S")

# 1) vst3_slot.rs: que la variable pueda forzar in-process (hoy yabridge siempre aísla)
p = "engine/src/vst3_slot.rs"
shutil.copy(p, p + ".bak_ll_" + ts)
s = open(p, encoding="utf-8").read()
pat = re.compile(r'[ \t]*let isolate = is_yabridge \|\| std::env::var_os\("LANZADERA_VST3_ISOLATE"\)\.is_some\(\);')
new = '''    // Aislamiento selectivo:
    //   LANZADERA_VST3_ISOLATE=1|all  -> todo aislado (máxima seguridad)
    //   LANZADERA_VST3_ISOLATE=0|none -> todo in-process (yabridge incluido:
    //      sin retardo IPC y con editor X11; asumes el riesgo de crash wine)
    //   sin variable (auto, default)  -> solo yabridge aislado
    let mode = std::env::var("LANZADERA_VST3_ISOLATE").unwrap_or_default();
    let isolate = match mode.as_str() {
        "1" | "all" | "yes" => true,
        "0" | "none" | "no" => false,
        _ => is_yabridge,
    };'''
if pat.search(s):
    s = pat.sub(new, s, count=1)
    open(p, "w", encoding="utf-8").write(s)
    print("✅ vst3_slot: aislamiento selectivo (0 = in-process)")
else:
    print("⚠ vst3_slot: no encontré la línea de isolate")

# 2) audio_io.rs: WINEPREFIX fuera (imprescindible para yabridge in-process)
p = "engine/src/audio_io.rs"
s = open(p, encoding="utf-8").read()
if "quitando WINEPREFIX" not in s:
    shutil.copy(p, p + ".bak_ll_" + ts)
    lines = s.split("\n")
    for i, ln in enumerate(lines):
        if "crate::clap_gui::install_alarm_handler();" in ln:
            indent = ln[:len(ln) - len(ln.lstrip())]
            block = [
                indent + "// yabridge deduce el prefijo wine de la ruta de cada plugin; un",
                indent + "// WINEPREFIX heredado del shell lo pisa y hace fallar IComponent::",
                indent + "// initialize (0x1). Imprescindible para yabridge IN-PROCESS.",
                indent + 'if std::env::var_os("LANZADERA_KEEP_WINEPREFIX").is_none() {',
                indent + '    if let Some(v) = std::env::var_os("WINEPREFIX") {',
                indent + '        println!("[lanzadera] quitando WINEPREFIX={:?} (yabridge auto-detecta por plugin)", v);',
                indent + '        std::env::remove_var("WINEPREFIX");',
                indent + "    }",
                indent + "}",
            ]
            lines[i+1:i+1] = block
            break
    else:
        raise SystemExit("⚠ audio_io: no encontré install_alarm_handler() como ancla")
    open(p, "w", encoding="utf-8").write("\n".join(lines))
    print("✅ audio_io: WINEPREFIX removal añadido")
else:
    print("ℹ audio_io: WINEPREFIX ya presente")
