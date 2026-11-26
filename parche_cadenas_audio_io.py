from pathlib import Path
import sys
p = Path("engine/src/audio_io.rs")
s = p.read_text(encoding="utf-8")

def rep(old, new, count=1, label=""):
    global s
    n = s.count(old)
    if n != count:
        print(f"✗ ancla '{label}': esperaba {count} coincidencia(s), hay {n}. No se escribió nada.")
        sys.exit(1)
    s = s.replace(old, new)
    print(f"✓ {label}")

# 1) campos del Renderer
rep('''    /// Note FX CLAP por pista: transforman las notas antes del instrumento.
    #[cfg(feature = "clap")]
    note_fx: [Option<Processor>; MAX_TRACKS],
    /// FX de audio CLAP por pista: procesan la mezcla de la pista (insert).
    #[cfg(feature = "clap")]
    fx_clap: [Option<Processor>; MAX_TRACKS],
    /// FX de audio VST3 por pista.
    #[cfg(feature = "vst3")]
    fx_vst3: [Option<vst3_slot::Vst3Slot>; MAX_TRACKS],
''', '''    /// Cadena de Note FX CLAP por pista: transforman las notas, en orden, antes del instrumento.
    #[cfg(feature = "clap")]
    note_fx: [Vec<Processor>; MAX_TRACKS],
    /// Cadena de FX de audio por pista (CLAP o VST3 mezclados), procesada en orden de inserción.
    fx_chains: [Vec<FxInst>; MAX_TRACKS],
''', label="campos Renderer")

# 2) enum FxInst + Drop
rep('''#[cfg(feature = "clap")]
impl Drop for Renderer {
    fn drop(&mut self) {
        if let Some(tx) = &self.retire {
            for slot in self.plugins.iter_mut().chain(self.note_fx.iter_mut()).chain(self.fx_clap.iter_mut()) {
                if let Some(p) = slot.take() { let _ = tx.send(p.into_stopped()); }
            }
        }
    }
}''', '''/// Un plugin FX dentro de la cadena de una pista.
#[allow(dead_code)]
enum FxInst {
    #[cfg(feature = "clap")]
    Clap(Processor),
    #[cfg(feature = "vst3")]
    Vst3(vst3_slot::Vst3Slot),
}

#[cfg(feature = "clap")]
impl Drop for Renderer {
    fn drop(&mut self) {
        if let Some(tx) = &self.retire {
            for slot in self.plugins.iter_mut() {
                if let Some(p) = slot.take() { let _ = tx.send(p.into_stopped()); }
            }
            for chain in self.note_fx.iter_mut() {
                for p in chain.drain(..) { let _ = tx.send(p.into_stopped()); }
            }
            for chain in self.fx_chains.iter_mut() {
                for fx in chain.drain(..) {
                    match fx {
                        FxInst::Clap(p) => { let _ = tx.send(p.into_stopped()); }
                        #[cfg(feature = "vst3")]
                        FxInst::Vst3(_) => {}
                    }
                }
            }
        }
    }
}''', label="enum FxInst + Drop")

# 3) Note FX en RT: bucle por la cadena
rep('''            if self.note_fx[t].is_none() || !self.has_plugin(t) { continue; }
            let Some(proc) = self.note_fx[t].as_mut() else { continue };
            let Ok(started) = proc.ensure_processing_started() else { continue };
            self.nfx_src.clear();''', '''            if self.note_fx[t].is_empty() || !self.has_plugin(t) { continue; }
            for ci in 0..self.note_fx[t].len() {
            let Some(proc) = self.note_fx[t].get_mut(ci) else { continue };
            let Ok(started) = proc.ensure_processing_started() else { continue };
            self.nfx_src.clear();''', label="Note FX apertura")
rep('''            crate::plugin_events::read_clap(&self.nfx_out, &mut self.pev[t]);
            self.pev[t].sort_by_frame();
        }
''', '''            crate::plugin_events::read_clap(&self.nfx_out, &mut self.pev[t]);
            self.pev[t].sort_by_frame();
            } // cadena Note FX
        }
''', label="Note FX cierre")

# 4) FX de audio en RT: un solo bucle sobre la cadena (orden real VST3/CLAP)
a = s.index('''        #[cfg(feature = "vst3")]
        for t in 0..n {
            let Some(slot) = self.fx_vst3[t].as_mut() else { continue; };''')
b = s.index('''        #[cfg(feature = "clap")]
        { self.steady = self.steady.wrapping_add(frames as u64); }''', a)
new_fx = '''        for t in 0..n {
            for ci in 0..self.fx_chains[t].len() {
                match &mut self.fx_chains[t][ci] {
                    #[cfg(feature = "vst3")]
                    FxInst::Vst3(slot) => {
                        if !vst3_slot::process(slot, &[], frames, &self.bufs[t][0][..frames], &self.bufs[t][1][..frames]) {
                            continue;
                        }
                        if slot.buffers.outputs.is_empty() { continue; }
                        let out_bus = &slot.buffers.outputs[0];
                        let ch_count = out_bus.channels.len().min(2);
                        if ch_count == 0 { continue; }
                        let buf = &mut self.bufs[t];
                        for f in 0..frames {
                            let l = out_bus.channels[0][f];
                            let r = if ch_count >= 2 { out_bus.channels[1][f] } else { l };
                            buf[0][f] = l;
                            buf[1][f] = r;
                        }
                    }
                    #[cfg(feature = "clap")]
                    FxInst::Clap(proc) => {
                        let Ok(started) = proc.ensure_processing_started() else { continue };
                        let ok = {
                            let [il, ir] = &mut *self.fxin;
                            il[..frames].copy_from_slice(&self.bufs[t][0][..frames]);
                            ir[..frames].copy_from_slice(&self.bufs[t][1][..frames]);
                            let [pl, pr] = &mut *self.pbuf;
                            pl[..frames].fill(0.0);
                            pr[..frames].fill(0.0);
                            let in_bufs = self.fx_ports.with_input_buffers([AudioPortBuffer {
                                latency: 0,
                                channels: AudioPortBufferType::f32_input_only([
                                    InputChannel::variable(&mut il[..frames]),
                                    InputChannel::variable(&mut ir[..frames]),
                                ].into_iter()),
                            }]);
                            let mut audio_out = self.out_ports.with_output_buffers([AudioPortBuffer {
                                latency: 0,
                                channels: AudioPortBufferType::f32_output_only(
                                    [&mut pl[..frames], &mut pr[..frames]].into_iter()
                                ),
                            }]);
                            self.nfx_in.clear();
                            let input_events = self.nfx_in.as_input();
                            let mut output_events = OutputEvents::void();
                            started.process(
                                &in_bufs, &mut audio_out,
                                &input_events, &mut output_events,
                                Some(self.steady), None,
                            ).is_ok()
                        };
                        if !ok { continue; } // si el efecto falla, ese slot no altera la señal
                        let buf = &mut self.bufs[t];
                        for f in 0..frames {
                            buf[0][f] = self.pbuf[0][f];
                            buf[1][f] = self.pbuf[1][f];
                        }
                    }
                    #[cfg(not(any(feature = "clap", feature = "vst3")))]
                    _ => {}
                }
            }
        }
'''
s = s[:a] + new_fx + s[b:]
print("✓ FX de audio RT: cadena en orden")

# 5) loaded: (pista, tipo, indice de slot, ruta)
rep("    loaded: Vec<(usize, u8, String)>,", "    loaded: Vec<(usize, u8, usize, String)>,", label="campo loaded")
rep("    let mut loaded: Vec<(usize, u8, String)> = Vec::new();", "    let mut loaded: Vec<(usize, u8, usize, String)> = Vec::new();", label="let loaded")
rep("loaded.push((t, 0, path.clone()));", "loaded.push((t, 0, 0, path.clone()));", count=2, label="loaded.push instrumento")

# 6) captura de estado por slot
a = s.index("        // (pista, ranura 0=instrumento 1=Note FX 2=FX, estado en base64)")
b = s.index("#[cfg(feature = \"clap\")]\nimpl AudioHandle {\n    /// Atiende la peticion", a)
new_cap = '''        // (pista, tipo 0=instrumento 1=Note FX 2=FX, índice de slot, estado en base64)
        #[allow(unused_mut)] // sin las features clap/vst3 no se añade nada
        let mut got: Vec<(usize, u8, usize, String)> = Vec::new();
        #[cfg(feature = "clap")]
        {
            for (t, inst) in self.instances.iter_mut() {
                if let Some(b) = clap_host::save_state(inst) {
                    got.push((*t, 0, 0, lanzadera_core::b64::encode(&b)));
                }
            }
            for (id, inst) in self.fx_instances.iter_mut() {
                if let Some((t, kind, i)) = crate::clap_gui::gui_id_slot(*id) {
                    if let Some(b) = clap_host::save_state(inst) {
                        got.push((t, kind, i, lanzadera_core::b64::encode(&b)));
                    }
                }
            }
        }
        #[cfg(feature = "vst3")]
        {
            for (t, h) in self.vst3_handles.iter() {
                if let Ok(pl) = h.lock() {
                    if let Ok(b) = pl.save_state() { got.push((*t, 0, 0, lanzadera_core::b64::encode(&b))); }
                }
            }
            for (id, h) in self.fx_vst3_handles.iter() {
                if let Some((t, kind, i)) = crate::clap_gui::gui_id_slot(*id) {
                    if let Ok(pl) = h.lock() {
                        if let Ok(b) = pl.save_state() { got.push((t, kind, i, lanzadera_core::b64::encode(&b))); }
                    }
                }
            }
        }
        if got.is_empty() { return; }
        let loaded = self.loaded.clone();
        shared.edit(|st| {
            for (t, kind, i, b) in got {
                let Some(tr) = st.tracks.get_mut(t) else { continue };
                let cur = match kind {
                    0 => tr.plugin_path.clone(),
                    1 => tr.note_fx_chain.get(i).map(|sl| sl.path.clone()),
                    _ => tr.fx_chain.get(i).map(|sl| sl.path.clone()),
                };
                if !loaded.iter().any(|(lt, lk, li, lp)| *lt == t && *lk == kind && *li == i && Some(lp) == cur.as_ref()) { continue; }
                match kind {
                    0 => tr.plugin_state = Some(b),
                    1 => { if let Some(sl) = tr.note_fx_chain.get_mut(i) { sl.state = Some(b); } }
                    _ => { if let Some(sl) = tr.fx_chain.get_mut(i) { sl.state = Some(b); } }
                }
            }
        });
    }
}

'''
s = s[:a] + new_cap + s[b:]
print("✓ captura de estado por slot")

# 7) limpieza de GUI al reiniciar: todos los huecos, no solo los 32 de instrumento
rep('''    #[cfg(feature = "clap")]
    for t in 0..MAX_TRACKS {
        crate::clap_gui::set_avail(t, false);
        crate::clap_gui::set_open(t, false);
    }''', '''    #[cfg(feature = "clap")]
    for t in 0..crate::clap_gui::MAX_PLUGIN_SLOTS {
        crate::clap_gui::set_avail(t, false);
        crate::clap_gui::set_open(t, false);
    }''', label="limpieza GUI")

# 8) cargador de cadenas
a = s.index('''    #[cfg(feature = "clap")]
    let mut note_fx: [Option<Processor>; MAX_TRACKS] = std::array::from_fn(|_| None);''')
b = s.index("    // ── Motor de drums de Caja (Fase 4A) ──", a)
new_load = '''    #[cfg(feature = "clap")]
    let mut note_fx: [Vec<Processor>; MAX_TRACKS] = std::array::from_fn(|_| Vec::new());
    #[allow(unused_mut)]
    let mut fx_chains: [Vec<FxInst>; MAX_TRACKS] = std::array::from_fn(|_| Vec::new());
    #[cfg(feature = "clap")]
    let mut fx_instances: Vec<(usize, clap_host::Instance)> = Vec::new();
    #[cfg(feature = "vst3")]
    let mut fx_vst3_handles: Vec<(usize, std::sync::Arc<Mutex<vst3_host::plugin::Plugin>>)> = Vec::new();
    {
        type Chain = Vec<lanzadera_core::track::PluginSlot>;
        let slots: Vec<(usize, Chain, Chain, bool)> = {
            let st = shared.state.lock().unwrap();
            st.tracks.iter().enumerate().take(MAX_TRACKS).map(|(i, tr)| (
                i,
                tr.note_fx_chain.clone(),
                tr.fx_chain.clone(),
                tr.plugin_path.is_some(),
            )).collect()
        };
        for (t, nchain, fchain, has_instr) in slots {
            if nchain.len() > crate::clap_gui::CHAIN_MAX || fchain.len() > crate::clap_gui::CHAIN_MAX {
                crate::clap_gui::notify(format!("⚠ Pista {}: máximo {} plugins por cadena; el resto se ignora", t + 1, crate::clap_gui::CHAIN_MAX));
            }
            if !has_instr && nchain.iter().any(|sl| !sl.bypass) {
                crate::clap_gui::notify(format!("⚠ Note FX pista {}: la pista no tiene instrumento plugin; no se aplica", t + 1));
            }
            for (i, slot) in nchain.iter().enumerate().take(crate::clap_gui::CHAIN_MAX) {
                if slot.bypass || !has_instr { continue; }
                if slot.path.trim_end_matches('/').ends_with(".vst3") {
                    crate::clap_gui::notify(format!("⚠ Note FX pista {}: los Note FX VST3 aún no se procesan; usa la versión CLAP", t + 1));
                    continue;
                }
                #[cfg(feature = "clap")]
                {
                    let gui_id = crate::clap_gui::nfx_gui_id(t, i);
                    match clap_host::load(&slot.path, slot.id.as_deref(), sample_rate, MAX_BLOCK as u32, gui_id) {
                        Ok((mut inst, proc, name, _params)) => {
                            println!("[lanzadera] Note FX {} pista {}: {}", i + 1, t + 1, name);
                            crate::clap_gui::probe(&mut inst, gui_id);
                            if let Some(bytes) = slot.state.as_deref().and_then(lanzadera_core::b64::decode) {
                                let _ = clap_host::load_state(&mut inst, &bytes);
                            }
                            loaded.push((t, 1, i, slot.path.clone()));
                            note_fx[t].push(proc);
                            fx_instances.push((gui_id, inst));
                        }
                        Err(e) => crate::clap_gui::notify(format!("⚠ Note FX {} pista {}: {}", i + 1, t + 1, e)),
                    }
                }
            }
            for (i, slot) in fchain.iter().enumerate().take(crate::clap_gui::CHAIN_MAX) {
                if slot.bypass { continue; }
                let gui_id = crate::clap_gui::fx_gui_id(t, i);
                if slot.path.trim_end_matches('/').ends_with(".vst3") {
                    #[cfg(feature = "vst3")]
                    match vst3_slot::load(&slot.path, sample_rate, MAX_BLOCK as u32) {
                        Ok(vslot) => {
                            if vslot.is_instrument {
                                crate::clap_gui::notify(format!("⚠ FX pista {}: «{}» es un instrumento, no un efecto", t + 1, vslot.name));
                            } else {
                                println!("[lanzadera] FX VST3 {} pista {}: {}", i + 1, t + 1, vslot.name);
                                #[cfg(feature = "clap")]
                                {
                                    crate::clap_gui::set_avail(gui_id, vslot.has_editor);
                                    crate::clap_gui::set_open(gui_id, false);
                                }
                                if let Some(bytes) = slot.state.as_deref().and_then(lanzadera_core::b64::decode) {
                                    if let Ok(mut pl) = vslot.plugin.lock() { let _ = pl.load_state(&bytes); }
                                }
                                loaded.push((t, 2, i, slot.path.clone()));
                                fx_vst3_handles.push((gui_id, vslot.plugin.clone()));
                                fx_chains[t].push(FxInst::Vst3(vslot));
                            }
                        }
                        Err(e) => crate::clap_gui::notify(format!("⚠ FX VST3 {} pista {}: {}", i + 1, t + 1, e)),
                    }
                } else {
                    #[cfg(feature = "clap")]
                    match clap_host::load(&slot.path, slot.id.as_deref(), sample_rate, MAX_BLOCK as u32, gui_id) {
                        Ok((mut inst, proc, name, _params)) => {
                            println!("[lanzadera] FX CLAP {} pista {}: {}", i + 1, t + 1, name);
                            crate::clap_gui::probe(&mut inst, gui_id);
                            if let Some(bytes) = slot.state.as_deref().and_then(lanzadera_core::b64::decode) {
                                let _ = clap_host::load_state(&mut inst, &bytes);
                            }
                            loaded.push((t, 2, i, slot.path.clone()));
                            fx_chains[t].push(FxInst::Clap(proc));
                            fx_instances.push((gui_id, inst));
                        }
                        Err(e) => crate::clap_gui::notify(format!("⚠ FX CLAP {} pista {}: {}", i + 1, t + 1, e)),
                    }
                }
            }
        }
    }

'''
s = s[:a] + new_load + s[b:]
print("✓ cargador de cadenas")

# 9) init del Renderer
rep('''        #[cfg(feature = "clap")]
        note_fx,
        #[cfg(feature = "clap")]
        fx_clap,
        #[cfg(feature = "vst3")]
        fx_vst3,
''', '''        #[cfg(feature = "clap")]
        note_fx,
        fx_chains,
''', label="init Renderer")

p.write_text(s, encoding="utf-8")
print("Todas las ediciones aplicadas.")
