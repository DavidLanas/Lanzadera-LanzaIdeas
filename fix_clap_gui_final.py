import shutil, datetime
p = "engine/src/clap_gui.rs"
shutil.copy(p, p + ".bak_fin_" + datetime.datetime.now().strftime("%Y%m%d_%H%M%S"))
lines = open(p, encoding="utf-8").read().split("\n")
out = []
a_alarm = a_field = a_init = False
for ln in lines:
    out.append(ln)
    if not a_alarm and "static ALARM_FIRED: AtomicBool" in ln:
        out.append("    /// Consume el flag de watchdog: true si la ultima llamada vigilada")
        out.append("    /// excedio TIMEOUT_SECS (aunque finalmente retornara).")
        out.append("    pub fn alarm_fired() -> bool { ALARM_FIRED.swap(false, Ordering::AcqRel) }")
        a_alarm = True
    if not a_field and ln.strip() == "pub open: [bool; MAX_TRACKS],":
        out.append("    /// La GUI de esta pista ya colgo una vez esta sesion: no reintentar.")
        out.append("    pub broken: [bool; MAX_TRACKS],")
        a_field = True
    if not a_init and ln.strip() == "open: [false; MAX_TRACKS],":
        out.append("    broken: [false; MAX_TRACKS],")
        a_init = True
open(p, "w", encoding="utf-8").write("\n".join(out))
print(f"alarm_fired fn: {a_alarm} | campo broken: {a_field} | init broken: {a_init}")
if not (a_alarm and a_field and a_init):
    print("⚠ alguna inserción falló; revisa el archivo")
