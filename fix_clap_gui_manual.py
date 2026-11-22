import re, shutil, datetime

p = "engine/src/clap_gui.rs"
shutil.copy(p, p + ".bak_m3_" + datetime.datetime.now().strftime("%Y%m%d_%H%M%S"))
s = open(p, encoding="utf-8").read()

def inject_after(pattern, inject, tag):
    global s
    if inject in s:
        print(f"ℹ {tag}: ya presente")
        return True
    m = re.search(pattern, s)
    if not m:
        print(f"⚠ {tag}: no encontrado")
        return False
    s = s[:m.end()] + "\n" + inject + s[m.end():]
    print(f"✅ {tag}")
    return True

# 1) ALARM_FIRED static en crash_guard
inject_after(
    r'pub\(super\) static IN_PLUGIN_GUI_CALL: AtomicBool = AtomicBool::new\(false\);',
    '    /// true si el último watchdog disparó (la llamada excedió TIMEOUT_SECS).\n    static ALARM_FIRED: AtomicBool = AtomicBool::new(false);',
    "ALARM_FIRED static"
)

# 2) Handler marca ALARM_FIRED
s = s.replace(
    'unsafe extern "C" fn alarm_noop(_sig: libc::c_int) {}',
    'unsafe extern "C" fn alarm_noop(_sig: libc::c_int) {\n            ALARM_FIRED.store(true, Ordering::Release);\n        }',
    1
)
print("✅ handler marca ALARM_FIRED")

# 3) Reset ALARM_FIRED en guarded
s = s.replace(
    '        IN_PLUGIN_GUI_CALL.store(true, Ordering::Release);\n        // Armar el timer: si el plugin bloquea TIMEOUT_SECS, SIGALRM interrumpe.\n        unsafe { libc::alarm(TIMEOUT_SECS); }',
    '        IN_PLUGIN_GUI_CALL.store(true, Ordering::Release);\n        ALARM_FIRED.store(false, Ordering::Release);\n        // Armar el timer: si el plugin bloquea TIMEOUT_SECS, SIGALRM interrumpe.\n        unsafe { libc::alarm(TIMEOUT_SECS); }',
    1
)
print("✅ reset ALARM_FIRED en guarded")

# 4) alarm_fired() fn al final de crash_guard
inject_after(
    r'pub fn guarded<F, R>\(f: F\) -> Result<R, String>.*?^\s*\}',
    '''
    /// Consume el flag de watchdog: true si la última llamada vigilada
    /// excedió TIMEOUT_SECS (aunque finalmente retornara).
    pub fn alarm_fired() -> bool { ALARM_FIRED.swap(false, Ordering::AcqRel) }''',
    "alarm_fired() en crash_guard"
)

# 5) guard_gui + gui_timed_out después de install_alarm_handler
inject_after(
    r'pub fn install_alarm_handler\(\) \{[^}]+\}',
    '''
/// Vigila una llamada de GUI en Linux (watchdog + catch_unwind); en otras
/// plataformas llama directamente.
#[cfg(feature = "clap")]
fn guard_gui<F>(f: F) -> Result<(), String>
where F: FnOnce() -> Result<(), String> + std::panic::UnwindSafe {
    #[cfg(target_os = "linux")]
    { crash_guard::guarded(f) }
    #[cfg(not(target_os = "linux"))]
    { f() }
}

/// true si el watchdog disparó durante la última llamada vigilada.
#[cfg(feature = "clap")]
fn gui_timed_out() -> bool {
    #[cfg(target_os = "linux")]
    { crash_guard::alarm_fired() }
    #[cfg(not(target_os = "linux"))]
    { false }
}''',
    "guard_gui + gui_timed_out"
)

# 6) Campo broken en GuiState
s = s.replace(
    '''pub struct GuiState {
    /// El plugin de esa pista expone la extension GUI.
    pub avail: [bool; MAX_TRACKS],
    /// La GUI de esa pista esta abierta ahora mismo.
    pub open: [bool; MAX_TRACKS],
}
pub static GUI_STATE: Mutex<GuiState> = Mutex::new(GuiState {
    avail: [false; MAX_TRACKS],
    open: [false; MAX_TRACKS],
});''',
    '''pub struct GuiState {
    /// El plugin de esa pista expone la extension GUI.
    pub avail: [bool; MAX_TRACKS],
    /// La GUI de esa pista esta abierta ahora mismo.
    pub open: [bool; MAX_TRACKS],
    /// La GUI de esta pista ya colgó una vez esta sesión: no reintentar.
    pub broken: [bool; MAX_TRACKS],
}
pub static GUI_STATE: Mutex<GuiState> = Mutex::new(GuiState {
    avail: [false; MAX_TRACKS],
    open: [false; MAX_TRACKS],
    broken: [false; MAX_TRACKS],
});''',
    1
)
print("✅ GuiState.broken")

# 7) set_broken + is_broken después de set_open
inject_after(
    r'pub fn set_open\(track: usize, v: bool\) \{[^}]+\}',
    '''pub fn set_broken(track: usize, v: bool) {
    if let Ok(mut s) = GUI_STATE.lock() {
        if track < MAX_TRACKS { s.broken[track] = v; }
    }
}
pub fn is_broken(track: usize) -> bool {
    GUI_STATE.lock().map(|s| s.broken[track]).unwrap_or(false)
}''',
    "set_broken/is_broken"
)

# 8) Check broken en toggle
s = s.replace(
    '''    let Some(gui) = main.get_extension::<PluginGui>() else {
        return Err("el plugin no expone interfaz grafica".into());
    };
    if state(track).1 {''',
    '''    let Some(gui) = main.get_extension::<PluginGui>() else {
        return Err("el plugin no expone interfaz grafica".into());
    };
    // Si este plugin ya colgó la GUI esta sesión, no reintentar: congelaría
    // otra vez el hilo principal. Sugerir VST3 o reinicio.
    if is_broken(track) {
        return Err("la GUI de este plugin ya colgó esta sesión (marcada como \
                    insegura). Usa su versión VST3 —las ventanas funcionan— o \
                    reinicia Lanzadera para reintentar.".into());
    }
    if state(track).1 {''',
    1
)
print("✅ broken-check en toggle")

# 9) Cierre vigilado
s = s.replace(
    '''    if state(track).1 {
        // Cierre: sin guard (hide+destroy son más seguros que create+show).
        let _ = gui.hide(&main);
        gui.destroy(&main);
        embed.remove(track); // la ventana host se destruye DESPUES de destroy()
        set_open(track, false);
        return Ok(());
    }''',
    '''    if state(track).1 {
        // Cierre vigilado: hide+destroy pueden bloquearse en plugins wine.
        let _ = guard_gui(std::panic::AssertUnwindSafe(|| {
            let _ = gui.hide(&main);
            gui.destroy(&main);
            Ok(())
        }));
        embed.remove(track); // la ventana host se destruye DESPUES de destroy()
        set_open(track, false);
        return Ok(());
    }''',
    1
)
print("✅ cierre vigilado")

# 10) Marcar broken si flotante timeout
s = s.replace(
    '''            Err(e) => {
                // show() falló o el plugin colgó: limpiar.
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gui.destroy(&main)));
                return Err(e);
            }''',
    '''            Err(e) => {
                // show() falló o el plugin colgó: limpiar.
                if gui_timed_out() { set_broken(track, true); }
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gui.destroy(&main)));
                return Err(e);
            }''',
    1
)
print("✅ broken si flotante timeout")

# 11) Marcar broken si create embebido timeout
s = s.replace(
    '    create_result?;',
    '''    if let Err(e) = create_result {
        if gui_timed_out() { set_broken(track, true); }
        return Err(e);
    }''',
    1
)
print("✅ broken si create embebido timeout")

# 12) set_parent + show embebido vigilados
s = s.replace(
    '''    let parent = unsafe { gui.set_parent(&main, Window::from_x11_handle(win.id() as std::ffi::c_ulong)) };
    if let Err(e) = parent {
        gui.destroy(&main);
        return Err(format!("set_parent: {:?}", e));
    }
    // En GUI embebida `show` es opcional: nih-plug y otros devuelven false
    // porque la vista ya es visible dentro de la ventana del host (que ya
    // esta mapeada). Por eso aqui un fallo de `show` NO es fatal.
    if let Err(e) = gui.show(&main) {
        println!("[gui] pista {}: show() de GUI embebida devolvio {:?} (se ignora)", track + 1, e);
    }''',
    '''    let parent_res = guard_gui(std::panic::AssertUnwindSafe(|| {
        unsafe { gui.set_parent(&main, Window::from_x11_handle(win.id() as std::ffi::c_ulong)) }
            .map_err(|e| format!("set_parent: {:?}", e))?;
        // En GUI embebida `show` es opcional: un fallo NO es fatal.
        if let Err(e) = gui.show(&main) {
            println!("[gui] pista {}: show() embebida devolvio {:?} (se ignora)", track + 1, e);
        }
        Ok(())
    }));
    if let Err(e) = parent_res {
        if gui_timed_out() { set_broken(track, true); }
        let _ = guard_gui(std::panic::AssertUnwindSafe(|| { gui.destroy(&main); Ok(()) }));
        embed.remove(track);
        return Err(e);
    }''',
    1
)
print("✅ set_parent+show embebido vigilados")

open(p, "w", encoding="utf-8").write(s)
print("\n✅ clap_gui.rs endurecido")
