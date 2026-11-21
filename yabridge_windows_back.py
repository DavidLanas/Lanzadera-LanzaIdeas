import re, shutil, datetime
ts = datetime.datetime.now().strftime("%Y%m%d_%H%M%S")

# ── 1) vst3_slot.rs: default in-process; aislar solo con =1 ──
p = "engine/src/vst3_slot.rs"
shutil.copy(p, p + ".bak_wb_" + ts)
s = open(p, encoding="utf-8").read()

old_iso = 'let isolate = is_yabridge || std::env::var_os("LANZADERA_VST3_ISOLATE").is_some();'
new_iso = '''// Default: TODO in-process (GUI X11 disponible y sin retardo IPC, como en
    // tus otros DAWs). El teardown cuelgue-privado vive en AudioHandle::drop
    // (hilo aparte), así que in-process ya no congela la app al cambiar plugin.
    // Válvula de escape: LANZADERA_VST3_ISOLATE=1 aísla todo (si un wine es
    // inestable y prefieres perder su ventana a riesgo de crash).
    let mode = std::env::var("LANZADERA_VST3_ISOLATE").unwrap_or_default();
    let isolate = matches!(mode.as_str(), "1" | "all" | "yes");'''
if old_iso in s:
    s = s.replace(old_iso, new_iso, 1)
elif 'let mode = std::env::var("LANZADERA_VST3_ISOLATE")' in s:
    s = re.sub(r'let mode = std::env::var\("LANZADERA_VST3_ISOLATE"\).*?_ => is_yabridge,\n    \};',
               new_iso, s, count=1, flags=re.S)
else:
    raise SystemExit("⚠ vst3_slot: no encontré la línea de isolate")

s = s.replace('"[vst3] yabridge detectado: {} — usando proceso aislado automáticamente"',
              '"[vst3] yabridge detectado: {} (in-process por defecto; =1 para aislar)"', 1)
open(p, "w", encoding="utf-8").write(s)
print("✅ vst3_slot: yabridge in-process por defecto (=1 para aislar)")

# ── 2) audio_io.rs: teardown VST3 en hilo aparte (anti-cuelgue al cambiar) ──
p = "engine/src/audio_io.rs"
shutil.copy(p, p + ".bak_wb_" + ts)
s = open(p, encoding="utf-8").read()
old_drop = '''            self.vst3_gui.close_all();
            for (t, _) in self.vst3_handles.iter() { crate::clap_gui::set_open(*t, false); }
        }
        // 1) Soltar el stream dropea el Renderer, que devuelve los procesadores.
        self.stream.take();
        // 1b) Con el Renderer ya muerto, esta es la ultima referencia a los
        //     plugins VST3: se destruyen aqui, en el hilo principal.
        #[cfg(feature = "vst3")]
        self.vst3_handles.clear();'''
new_drop = '''            for (t, _) in self.vst3_handles.iter() { crate::clap_gui::set_open(*t, false); }
        }
        // 1) Soltar el stream dropea el Renderer, que devuelve los procesadores.
        self.stream.take();
        // 1b) Teardown VST3 en HILO APARTE: cerrar un editor o destruir un
        //     plugin yabridge/wine puede bloquearse (teardown del bridge).
        //     En el hilo principal congelaba la app al cambiar de plugin;
        //     en un hilo suelto, un bloqueo se queda ahí y la app sigue.
        #[cfg(feature = "vst3")]
        {
            let mut gui = std::mem::take(&mut self.vst3_gui);
            let handles = std::mem::take(&mut self.vst3_handles);
            std::thread::Builder::new()
                .name("vst3-teardown".into())
                .spawn(move || {
                    gui.close_all();  // editores primero (la vista antes que el plugin)
                    drop(handles);    // luego los plugins (teardown wine)
                })
                .ok();
        }'''
if old_drop in s:
    s = s.replace(old_drop, new_drop, 1)
    print("✅ audio_io: teardown VST3 en hilo aparte")
elif "vst3-teardown" in s:
    print("ℹ audio_io: teardown ya era en hilo aparte")
else:
    print("⚠ audio_io: no encontré el bloque Drop VST3; revisa a mano")

# ── 3) audio_io.rs: WINEPREFIX fuera (imprescindible para yabridge in-process) ──
if "quitando WINEPREFIX" not in s:
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
        print("✅ audio_io: WINEPREFIX removal añadido")
    else:
        print("⚠ audio_io: sin ancla install_alarm_handler; añade WINEPREFIX a mano")
else:
    print("ℹ audio_io: WINEPREFIX presente")
open(p, "w", encoding="utf-8").write(s)
