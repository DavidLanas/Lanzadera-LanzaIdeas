import shutil, datetime
p = "engine/src/clap_gui.rs"
shutil.copy(p, p + ".bak_gh_" + datetime.datetime.now().strftime("%Y%m%d_%H%M%S"))
s = open(p, encoding="utf-8").read()
def rep(old, new, tag):
    global s
    if old in s:
        s = s.replace(old, new, 1); print("✅", tag)
    else:
        print("⚠ no coincidió:", tag)

# 1) Watchdog: registrar qué llamada disparó el alarm
rep('''    /// true mientras hay una llamada al plugin en curso (para diagnóstico).
    pub(super) static IN_PLUGIN_GUI_CALL: AtomicBool = AtomicBool::new(false);''',
    '''    /// true mientras hay una llamada al plugin en curso (para diagnóstico).
    pub(super) static IN_PLUGIN_GUI_CALL: AtomicBool = AtomicBool::new(false);
    /// true si el último watchdog disparó (la llamada excedió TIMEOUT_SECS).
    static ALARM_FIRED: AtomicBool = AtomicBool::new(false);''', "ALARM_FIRED static")

rep('''        unsafe extern "C" fn alarm_noop(_sig: libc::c_int) {}''',
    '''        unsafe extern "C" fn alarm_noop(_sig: libc::c_int) {
            ALARM_FIRED.store(true, Ordering::Release);
        }''', "handler marca ALARM_FIRED")

rep('''        IN_PLUGIN_GUI_CALL.store(true, Ordering::Release);
        // Armar el timer: si el plugin bloquea TIMEOUT_SECS, SIGALRM interrumpe.
        unsafe { libc::alarm(TIMEOUT_SECS); }''',
    '''        IN_PLUGIN_GUI_CALL.store(true, Ordering::Release);
        ALARM_FIRED.store(false, Ordering::Release);
        // Armar el timer: si el plugin bloquea TIMEOUT_SECS, SIGALRM interrumpe.
        unsafe { libc::alarm(TIMEOUT_SECS); }''', "reset ALARM_FIRED en guarded")

rep('''                 "el plugin sufrió un panic durante la apertura de GUI \
                  (posible bug del plugin; reinicia Lanzadera si hay audio corrupto)"
            )),
        }
    }
}''',
    '''                 "el plugin sufrió un panic durante la apertura de GUI \
                  (posible bug del plugin; reinicia Lanzadera si hay audio corrupto)"
            )),
        }
    }

    /// Consume el flag de watchdog: true si la última llamada vigilada
    /// excedió TIMEOUT_SECS (aunque finalmente retornara).
    pub fn alarm_fired() -> bool { ALARM_FIRED.swap(false, Ordering::AcqRel) }
}''', "alarm_fired()")

# 2) Wrappers disponibles en toda la módulo
rep('''pub fn install_alarm_handler() {
    #[cfg(all(feature = "clap", target_os = "linux"))]
    crash_guard::install_alarm_handler();
}''',
    '''pub fn install_alarm_handler() {
    #[cfg(all(feature = "clap", target_os = "linux"))]
    crash_guard::install_alarm_handler();
}

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
}''', "guard_gui + gui_timed_out")

# 3) Estado: pista marcada como "GUI colgante"
rep('''pub struct GuiState {
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
});''', "GuiState.broken")

rep('''pub fn set_open(track: usize, v: bool) {
    if let Ok(mut s) = GUI_STATE.lock() {
        if track < MAX_TRACKS { s.open[track] = v; }
    }
}''',
    '''pub fn set_open(track: usize, v: bool) {
    if let Ok(mut s) = GUI_STATE.lock() {
        if track < MAX_TRACKS { s.open[track] = v; }
    }
}
pub fn set_broken(track: usize, v: bool) {
    if let Ok(mut s) = GUI_STATE.lock() {
        if track < MAX_TRACKS { s.broken[track] = v; }
    }
}
pub fn is_broken(track: usize) -> bool {
    GUI_STATE.lock().map(|s| s.broken[track]).unwrap_or(false)
}''', "set_broken/is_broken")

# 4) toggle: no reintentar una GUI marcada como colgante
rep('''    // Negociacion, como pide la secuencia documentada de CLAP: primero se''',
    '''    // Si este plugin ya colgó la GUI esta sesión, no reintentar: congelaría
    // otra vez el hilo principal. Sugerir VST3 o reinicio.
    if is_broken(track) {
        return Err("la GUI de este plugin ya colgó esta sesión (marcada como \
                    insegura). Usa su versión VST3 —las ventanas funcionan— o \
                    reinicia Lanzadera para reintentar.".into());
    }
    // Negociacion, como pide la secuencia documentada de CLAP: primero se''', "broken-check en toggle")

# 5) Cierre vigilado (hide+destroy)
rep('''        let _ = gui.hide(&main);
        gui.destroy(&main);
        embed.remove(track); // la ventana host se destruye DESPUES de destroy()
        set_open(track, false);
        return Ok(());''',
    '''        let _ = guard_gui(std::panic::AssertUnwindSafe(|| {
            let _ = gui.hide(&main);
            gui.destroy(&main);
            Ok(())
        }));
        embed.remove(track); // la ventana host se destruye DESPUES de destroy()
        set_open(track, false);
        return Ok(());''', "cierre vigilado")

# 6) Marcar broken si la flotante colgó (timeout) en vez de solo fallar
rep('''            Err(e) => {
                // show() falló o el plugin colgó: limpiar.
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gui.destroy(&main)));
                return Err(e);
            }''',
    '''            Err(e) => {
                // show() falló o el plugin colgó: limpiar.
                if gui_timed_out() { set_broken(track, true); }
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gui.destroy(&main)));
                return Err(e);
            }''', "broken si flotante timeout")

# 7) create embebido: marcar broken si timeout
rep('''    create_result?;''',
    '''    if let Err(e) = create_result {
        if gui_timed_out() { set_broken(track, true); }
        return Err(e);
    }''', "broken si create embebido timeout")

# 8) set_parent + show embebido vigilados
rep('''    let parent = unsafe { gui.set_parent(&main, Window::from_x11_handle(win.id() as std::ffi::c_ulong)) };
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
    }''', "set_parent+show embebido vigilados")

open(p, "w", encoding="utf-8").write(s)
print("\n✅ clap_gui.rs endurecido")
