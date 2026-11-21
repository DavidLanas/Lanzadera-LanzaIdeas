#!/usr/bin/env bash
set -uo pipefail
cd "$(cd "$(dirname "$0")" && pwd)"

# 1) Marcar vendor/vst3-host como excluido del workspace raíz (idempotente).
#    Esto NO cambia tu build normal: sigue siendo un [patch.crates-io] path dep.
python3 - << 'PY'
import re
p = "Cargo.toml"
s = open(p, encoding="utf-8").read()
if "exclude" not in s.split("[profile.release]")[0]:
    s = s.replace(
        'members = ["core", "engine", "controller", "ui", "app"]',
        'members = ["core", "engine", "controller", "ui", "app"]\nexclude = ["vendor/vst3-host"]',
        1)
    open(p, "w", encoding="utf-8").write(s)
    print("✅ workspace.exclude añadido para vendor/vst3-host")
else:
    print("ℹ workspace.exclude ya estaba")
PY

# 2) Compilar helper + probe en su propio workspace (sin cpal-backend ni egui).
echo "→ Compilando vst3-host-helper y vst3-host-probe..."
cargo build --manifest-path vendor/vst3-host/Cargo.toml --bins \
  --no-default-features --features process-isolation || exit 1

# 3) Copiarlos junto al ejecutable de Lanzadera (donde el crate los busca).
mkdir -p target/debug
cp -f vendor/vst3-host/target/debug/vst3-host-helper target/debug/ || exit 1
cp -f vendor/vst3-host/target/debug/vst3-host-probe  target/debug/ 2>/dev/null || true

echo
ls -l target/debug/vst3-host-helper target/debug/vst3-host-probe 2>/dev/null
echo
echo "✅ Listo. Ahora ejecuta:"
echo "  LANZADERA_VST3_ISOLATE=1 cargo run --bin lanzadera --features clap,vst3"
