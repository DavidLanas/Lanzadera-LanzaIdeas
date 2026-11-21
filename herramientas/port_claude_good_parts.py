import shutil, datetime
ts = datetime.datetime.now().strftime("%Y%m%d_%H%M%S")

# ═══ 1) clap_gui.rs: crash_guard + install_alarm_handler + toggle protegido ═══
p = "engine/src/clap_gui.rs"
shutil.copy(p, p + ".bak_cg_" + ts)
s = open(p, encoding="utf-8").read()

guard = '''/// Guard de llamadas al plugin: SIGALRM si el plugin bloquea > TIMEOUT_SECS
/// y catch_unwind para panics de Rust. No protege de SIGSEGV (eso requiere
/// proceso separado, ver LANZADERA_VST3_ISOLATE), pero sí de deadlocks y panics.
/// (Portado del análisis de LYNX-SHARE PluginSandbox; aquí alarm() de POSIX.)
#[cfg(all(feature = "clap", target_os = "linux"))]
mod crash_guard {
    use std::sync::atomic::{AtomicBool, Ordering};
    /// Segundos máximos para gui.create()/gui.show().
    const TIMEOUT_SECS: libc::c_uint = 6;
    #[allow(dead_code)]
    pub(super) static IN_PLUGIN_GUI_CALL: AtomicBool = AtomicBool::new(false);
    /// Handler vacío: solo interrumpe la syscall bloqueada del plugin (EINTR).
    pub fn install_alarm_handler() {
        use std::sync::Once;
        static INSTALLED: Once = Once::new();
        INSTALLED.call_once(|| {
            unsafe extern "C" fn alarm_noop(_sig: libc::c_int) {}
            unsafe { libc::signal(libc::SIGALRM, alarm_noop as libc::sighandler_t); }
        });
    }
    pub fn guarded<F, R>(f: F) -> Result<R, String>
    where F: FnOnce() -> Result<R, String> + std::panic::UnwindSafe {
        IN_PLUGIN_GUI_CALL.store(true, Ordering::Release);
        unsafe { libc::alarm(TIMEOUT_SECS); }
        let result = std::panic::catch_unwind(f);
        unsafe { libc::alarm(0); }
        IN_PLUGIN_GUI_CALL.store(false, Ordering::Release);
        match result {
            Ok(r) => r,
            Err(_) => Err("el plugin sufrió un panic al abrir la GUI \
                 (bug del plugin; reinicia Lanzadera si hay audio corrupto)".to_string()),
        }
    }
}

/// Instala el watchdog SIGALRM para GUI de plugins CLAP. No-op fuera de Linux/clap.
pub fn install_alarm_handler() {
    #[cfg(all(feature = "clap", target_os = "linux"))]
    crash_guard::install_alarm_handler();
}

'''
if "mod crash_guard" not in s:
    anchor = "/// Abre o cierra la GUI de una pista"
    s = s.replace(anchor, guard + anchor, 1)

# Flotante protegida
old_float = '''    if use_float {
        gui.create(&main, cfg).map_err(|e| format!("create: {:?}", e))?;
        let ctitle = std::ffi::CString::new(title)
            .unwrap_or_else(|_| std::ffi::CString::new("plugin").unwrap());
        gui.suggest_title(&main, &ctitle);
        if let Err(e) = gui.show(&main) {
            gui.destroy(&main);
            return Err(format!("show: {:?}", e));
        }
        set_open(track, true);
        return Ok(());
    }'''
new_float = '''    if use_float {
        #[cfg(target_os = "linux")]
        let result = crash_guard::guarded(std::panic::AssertUnwindSafe(|| {
            gui.create(&main, cfg).map_err(|e| format!("create: {:?}", e))?;
            let ctitle = std::ffi::CString::new(title)
                .unwrap_or_else(|_| std::ffi::CString::new("plugin").unwrap());
            gui.suggest_title(&main, &ctitle);
            gui.show(&main).map_err(|e| format!("show: {:?}", e))
        }));
        #[cfg(not(target_os = "linux"))]
        let result = {
            gui.create(&main, cfg).map_err(|e| format!("create: {:?}", e))?;
            let ctitle = std::ffi::CString::new(title)
                .unwrap_or_else(|_| std::ffi::CString::new("plugin").unwrap());
            gui.suggest_title(&main, &ctitle);
            gui.show(&main).map_err(|e| format!("show: {:?}", e))
        };
        match result {
            Ok(()) => { set_open(track, true); return Ok(()); }
            Err(e) => {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gui.destroy(&main)));
                return Err(e);
            }
        }
    }'''
if old_float in s:
    s = s.replace(old_float, new_float, 1)
else:
    print("⚠ no encontré la rama flotante exacta; revisa a mano")

# Embebida protegida
old_emb = '    gui.create(&main, embedded).map_err(|e| format!("create(embebida): {:?}", e))?;'
new_emb = '''    #[cfg(target_os = "linux")]
    let create_result = crash_guard::guarded(std::panic::AssertUnwindSafe(|| {
        gui.create(&main, embedded).map_err(|e| format!("create(embebida): {:?}", e))
    }));
    #[cfg(not(target_os = "linux"))]
    let create_result = gui.create(&main, embedded).map_err(|e| format!("create(embebida): {:?}", e));
    create_result?;'''
if old_emb in s:
    s = s.replace(old_emb, new_emb, 1)
else:
    print("⚠ no encontré el create embebido exacto; revisa a mano")
open(p, "w", encoding="utf-8").write(s)
print("✅ clap_gui.rs: crash_guard + install_alarm_handler + toggle protegido")

# ═══ 2) vst3_slot.rs: detector yabridge robusto + pre-chequeo de wine ═══
p = "engine/src/vst3_slot.rs"
shutil.copy(p, p + ".bak_yb_" + ts)
s = open(p, encoding="utf-8").read()

det = '''/// Detecta si un bundle VST3 es un proxy yabridge leyendo
/// `<bundle>/Contents/x86_64-linux/libyabridge*.so` (más fiable que el nombre).
fn is_yabridge_bundle(bundle: &Path) -> bool {
    let so_dir = bundle.join("Contents/x86_64-linux");
    let Ok(rd) = std::fs::read_dir(&so_dir) else { return false };
    rd.flatten().any(|e| {
        let n = e.file_name().to_string_lossy().into_owned();
        n.starts_with("libyabridge") && n.ends_with(".so")
    })
}

'''
if "fn is_yabridge_bundle" not in s:
    s = s.replace("/// Carga y activa un plugin VST3.", det + "/// Carga y activa un plugin VST3.", 1)

check = '''    let is_yabridge = is_yabridge_bundle(&bundle) || bundle.to_string_lossy().contains("yabridge");
    if is_yabridge {
        let wine_ok = std::process::Command::new("wine")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|st| st.success())
            .unwrap_or(false);
        if !wine_ok {
            return Err(format!(
                "plugin yabridge '{}': wine no encontrado en PATH. \\
                 Instala wine y ejecuta `yabridgectl sync`.",
                bundle.file_stem().unwrap_or_default().to_string_lossy()
            ));
        }
        println!("[vst3] yabridge detectado: {} — aislamiento automático", bundle.display());
    }
'''
anchor = '''    let bundle = resolve_bundle(path);
    if !bundle.exists() {
        return Err(format!("no existe: {}", bundle.display()));
    }
'''
if "is_yabridge detectado" not in s:
    s = s.replace(anchor, anchor + check, 1)

# Sustituir el detector crudo del bloque de modo por el ya calculado arriba
old_crude = '    let is_yabridge = bundle.to_string_lossy().contains("yabridge");'
if old_crude in s:
    s = s.replace(old_crude, '    // is_yabridge ya calculado arriba con el detector robusto.', 1)
open(p, "w", encoding="utf-8").write(s)
print("✅ vst3_slot.rs: detector yabridge robusto + pre-chequeo wine")
