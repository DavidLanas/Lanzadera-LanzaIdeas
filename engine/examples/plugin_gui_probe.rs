//! Prueba SIN audio de la carga + ventana de un plugin (VST3 o CLAP).
//!
//!   cargo run -p lanzadera-engine --features clap,vst3 --example plugin_gui_probe -- /ruta/Plugin.vst3
//!   cargo run -p lanzadera-engine --features clap,vst3 --example plugin_gui_probe -- /ruta/Plugin.clap
//!
//! Imprime el resultado de cada paso (escaneo, carga, editor) y mantiene la
//! ventana abierta unos segundos bombeando el run loop como hace la app.
use std::time::{Duration, Instant};

fn main() {
    let arg = std::env::args().nth(1);
    let secs: u64 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(6);

    println!("scan VST3 -> {} plugins", lanzadera_engine::vst3_scan().len());
    for (p, n) in lanzadera_engine::vst3_scan() { println!("   {n}  ({p})"); }
    println!("scan CLAP -> {} plugins", lanzadera_engine::clap_scan().len());
    for (p, id, n) in lanzadera_engine::clap_scan() { println!("   {n} [{id}]  ({p})"); }
    let Some(path) = arg else { return };

    if path.trim_end_matches('/').ends_with(".vst3") {
        use lanzadera_engine::vst3_slot;
        let slot = match vst3_slot::load(&path, 48000, 512) {
            Ok(s) => s, Err(e) => { println!("LOAD ERR: {e}"); return; }
        };
        println!("VST3 OK: {} params={} editor={}", slot.name, slot.params.len(), slot.has_editor);
        let handles = vec![(0usize, slot.plugin.clone())];
        let mut gui = vst3_slot::Vst3Gui::default();
        match gui.toggle(0, &slot.plugin) {
            Ok(o) => println!("editor VST3 abierto={o}"),
            Err(e) => { println!("EDITOR ERR: {e}"); return; }
        }
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(secs) {
            let closed = gui.poll(&handles);
            if !closed.is_empty() { println!("cerrado por el usuario"); break; }
            std::thread::sleep(Duration::from_millis(16));
        }
        gui.close_all();
        drop(handles);
        println!("fin");
    } else {
        use lanzadera_engine::{clap_gui, clap_host};
        let (mut inst, proc, name, params) = match clap_host::load(&path, None, 48000, 512, 0) {
            Ok(x) => x, Err(e) => { println!("LOAD ERR: {e}"); return; }
        };
        println!("CLAP OK: {name} params={}", params.len());
        clap_gui::probe(&mut inst, 0);
        println!("gui avail/open = {:?}", clap_gui::state(0));
        let mut embed = clap_gui::EmbedHost::default();
        let mut insts = vec![(0usize, inst)];
        if let Err(e) = clap_gui::toggle(&mut insts[0].1, 0, "probe", &mut embed) {
            println!("EDITOR ERR: {e}"); return;
        }
        println!("gui avail/open = {:?}", clap_gui::state(0));
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(secs) {
            clap_gui::poll_embedded(&mut insts, &mut embed);
            std::thread::sleep(Duration::from_millis(16));
        }
        clap_gui::close_all(&mut insts, &mut embed);
        let (_, mut inst) = insts.pop().unwrap();
        inst.deactivate(proc.into_stopped());
        println!("fin");
    }
}
