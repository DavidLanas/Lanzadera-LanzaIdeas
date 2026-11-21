import shutil, datetime, re
p = "engine/src/audio_io.rs"
shutil.copy(p, p + ".bak_td_" + datetime.datetime.now().strftime("%Y%m%d_%H%M%S"))
s = open(p, encoding="utf-8").read()

old = '''        #[cfg(feature = "vst3")]
        {
            // El editor VST3 debe soltarse antes que el plugin (y su ventana
            // padre despues de la vista).
            self.vst3_gui.close_all();
            for (t, _) in self.vst3_handles.iter() { crate::clap_gui::set_open(*t, false); }
        }
        // 1) Soltar el stream dropea el Renderer, que devuelve los procesadores.
        self.stream.take();
        // 1b) Con el Renderer ya muerto, esta es la ultima referencia a los
        //     plugins VST3: se destruyen aqui, en el hilo principal.
        #[cfg(feature = "vst3")]
        self.vst3_handles.clear();'''

new = '''        #[cfg(feature = "vst3")]
        {
            for (t, _) in self.vst3_handles.iter() { crate::clap_gui::set_open(*t, false); }
        }
        // 1) Soltar el stream dropea el Renderer, que devuelve los procesadores.
        self.stream.take();
        // 1b) Teardown VST3 en HILO APARTE: cerrar un editor o destruir un
        //     plugin yabridge/wine puede bloquearse (teardown del bridge).
        //     Hacerlo en el hilo principal congelaba la app al cambiar de
        //     plugin. En un hilo suelto, un bloqueo se queda ahí: la app
        //     sigue viva con el nuevo handle de audio.
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

if old not in s:
    raise SystemExit("❌ no encontré el bloque Drop de VST3 tal cual; revisa a mano")
s = s.replace(old, new, 1)
open(p, "w", encoding="utf-8").write(s)
print("✅ Drop de AudioHandle: teardown VST3 en hilo aparte (anti-congelación)")
