import os, re, shutil, datetime

if not os.path.isdir("engine/src"):
    print("❌ Ejecuta desde la raíz de Lanzadera-LanzaIdeas"); exit(1)

ts = datetime.datetime.now().strftime("%Y%m%d_%H%M%S")
def backup(p): shutil.copy(p, p + f".bak_{ts}")

def replace_fn(src, sig, new_fn):
    i = src.find(sig)
    if i == -1: raise SystemExit(f"❌ no encontré: {sig}")
    start = src.find("{", i); depth = 0; j = start; end = None
    while j < len(src):
        if src[j] == "{": depth += 1
        elif src[j] == "}":
            depth -= 1
            if depth == 0: end = j + 1; break
        j += 1
    if end is None: raise SystemExit("❌ fin de función no encontrado")
    return src[:i] + new_fn + src[end:]

# ── 1) vst3_slot.rs: process con block_size correcto + entrada de efectos ──
p = "engine/src/vst3_slot.rs"; backup(p)
s = open(p, encoding="utf-8").read()

new_process = '''pub fn process(slot: &mut Vst3Slot, events: &[PluginEv], frames: usize, in_l: &[f32], in_r: &[f32]) -> bool {
    let Some(mut plugin) = lock_for_audio(&slot.plugin) else {
        for e in events {
            if slot.pending.len() < slot.pending.capacity() {
                let mut e = *e;
                e.frame = 0;
                slot.pending.push(e);
            }
        }
        return false;
    };
    // El tamaño real lo fija el callback. Se acota por la CAPACIDAD reservada
    // (no por block_size, que cambia cada bloque) para no reasignar en RT.
    let cap = slot.buffers.outputs.iter().chain(slot.buffers.inputs.iter())
        .flat_map(|b| b.channels.iter())
        .map(|c| c.capacity())
        .min()
        .unwrap_or(frames);
    let n = frames.min(cap).max(1);
    for bus in slot.buffers.inputs.iter_mut().chain(slot.buffers.outputs.iter_mut()) {
        for ch in bus.channels.iter_mut() {
            if ch.len() != n { ch.resize(n, 0.0); }
        }
    }
    slot.buffers.clear();
    // CRÍTICO: el plugin toma numSamples de block_size. Si se queda en MAX_BLOCK
    // (4096) y el callback entrega p.ej. 256, procesa 4096 sobre buffers de 256:
    // sonido saturado/comprimido "como con poco buffer" y posible corrupción.
    slot.buffers.block_size = n;

    // Alimentar buses de entrada activos (efectos). Mono -> estéreo si hace falta.
    if !in_l.is_empty() || !in_r.is_empty() {
        for bus in slot.buffers.inputs.iter_mut() {
            if !bus.active { continue; }
            for (ci, ch) in bus.channels.iter_mut().enumerate() {
                let src = if ci == 1 && !in_r.is_empty() { in_r } else { in_l };
                let m = ch.len().min(src.len());
                ch[..m].copy_from_slice(&src[..m]);
            }
        }
    }

    for e in slot.pending.iter() { deliver(&mut plugin, e); }
    slot.pending.clear();
    for e in events { deliver(&mut plugin, e); }
    plugin.process_bus_audio(&mut slot.buffers).is_ok()
}'''
s = replace_fn(s, "pub fn process(", new_process)

# Cadena de errores completa (yabridge anida causas)
helper = '''
/// Formatea un error con toda su cadena de causas (yabridge/wine anidan errores).
fn err_chain(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut src = e.source();
    while let Some(c) = src {
        s.push_str(" <- ");
        s.push_str(&c.to_string());
        src = c.source();
    }
    s
}

'''
if "fn err_chain(" not in s:
    s = s.replace("/// Carga y activa un plugin VST3.", helper + "/// Carga y activa un plugin VST3.", 1)
s = s.replace('format!("load_plugin({}): {}", bundle.display(), e)',
              'format!("load_plugin({}): {}", bundle.display(), err_chain(&e))')
s = s.replace('format!("create_bus_audio_buffers: {}", e)',
              'format!("create_bus_audio_buffers: {}", err_chain(&e))')
s = s.replace('format!("start_processing: {}", e)',
              'format!("start_processing: {}", err_chain(&e))')
open(p, "w", encoding="utf-8").write(s)
print("✅ vst3_slot.rs: block_size sincronizado + entrada de efectos + errores completos")

# ── 2) audio_io.rs: rutear entrada de efectos y sustituir dry (no sumar) ──
p = "engine/src/audio_io.rs"; backup(p)
s = open(p, encoding="utf-8").read()

new_block = '''        // ── Plugins VST3: un process_bus_audio() por pista y bloque ──
        // Efectos: la señal de la pista entra por los buses de audio del plugin
        // y la salida sustituye al dry (no se suma dos veces). Instrumentos:
        // generan desde MIDI y su salida se suma a la pista.
        #[cfg(feature = "vst3")]
        for t in 0..n {
            let Some(slot) = self.vst3_plugins[t].as_mut() else { continue; };
            let is_fx = !slot.is_instrument;
            let (in_l, in_r): (&[f32], &[f32]) = if is_fx {
                (&self.bufs[t][0][..frames], &self.bufs[t][1][..frames])
            } else {
                (&[], &[])
            };
            if !vst3_slot::process(slot, self.pev[t].as_slice(), frames, in_l, in_r) { continue; }

            if !slot.buffers.outputs.is_empty() {
                let out_bus = &slot.buffers.outputs[0];
                let buf = &mut self.bufs[t];
                if is_fx {
                    for f in 0..frames { buf[0][f] = 0.0; buf[1][f] = 0.0; }
                }
                let ch_count = out_bus.channels.len().min(2);
                for f in 0..frames {
                    if ch_count >= 1 { buf[0][f] += out_bus.channels[0][f]; }
                    if ch_count >= 2 { buf[1][f] += out_bus.channels[1][f]; }
                    else             { buf[1][f] += out_bus.channels[0][f]; }
                }
            }
        }

'''
pat = re.compile(r"        // ── Plugins VST3.*?(?=        // ── Plugins CLAP)", re.DOTALL)
if not pat.search(s):
    raise SystemExit("❌ no encontré el bloque VST3 en audio_io.rs")
s = pat.sub(new_block, s, count=1)
open(p, "w", encoding="utf-8").write(s)
print("✅ audio_io.rs: efectos alimentados y dry sustituido por wet")
