#!/usr/bin/env bash
set -uo pipefail
cd "$(cd "$(dirname "$0")" && pwd)"
echo "→ Construyendo vst3-host-helper (hosting en proceso aislado)..."
if cargo build -p vst3-host --bins --no-default-features --features process-isolation; then
  echo "✅ helper construido en target/debug (via -p)"
else
  echo "⚠ -p no disponible; construyendo via manifest-path y copiando..."
  cargo build --manifest-path vendor/vst3-host/Cargo.toml --bins --no-default-features --features process-isolation || exit 1
  mkdir -p target/debug
  cp -f vendor/vst3-host/target/debug/vst3-host-helper target/debug/ || exit 1
  cp -f vendor/vst3-host/target/debug/vst3-host-probe target/debug/ 2>/dev/null || true
  echo "✅ helper copiado a target/debug"
fi
echo
ls -l target/debug/vst3-host-helper 2>/dev/null || { echo "❌ helper no encontrado"; exit 1; }
echo
echo "Ahora ejecuta:"
echo "  LANZADERA_VST3_ISOLATE=1 cargo run --bin lanzadera --features clap,vst3"
