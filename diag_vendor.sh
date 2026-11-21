#!/usr/bin/env bash
cd "$(cd "$(dirname "$0")" && pwd)"
V=vendor/vst3-host/src
{
echo "================ 1) GUARD DEL DISPATCHER macOS ================"
grep -rn "macOS main-thread dispatcher" "$V" || echo "(no encontrado)"
grep -rn "macOS main-thread dispatcher" "$V" | head -5 | while IFS=: read -r f l _; do
  echo "### $f:$l"
  awk -v n="$l" 'NR>=n-30 && NR<=n+8 {printf "%5d  %s\n", NR, $0}' "$f"
done

echo
echo "================ 2) API / ESTADO DEL DISPATCHER ================"
grep -rn -iE "dispatcher" "$V" | grep -iE "fn |thread_local|static |pub |install|set_|with_" | head -50

echo
echo "================ 3) SPAWN DEL HELPER (env / Command) ================"
F="$V/process_isolation.rs"
grep -n -E "Command::new|env_clear|\.envs?\(|current_dir|helper|spawn" "$F" | head -50
grep -n "Command::new" "$F" | head -2 | while IFS=: read -r l _; do
  echo "### $F:$l"
  awk -v n="$l" 'NR>=n-20 && NR<=n+45 {printf "%5d  %s\n", NR, $0}' "$F"
done

echo
echo "================ 4) RUTA DE EDITOR (plugin.rs / window.rs) ================"
grep -rn -E "fn open_editor|fn open\b|X11EmbedWindowID|attached\(|require.*main_thread|dispatcher|Failed to open editor" "$V/plugin.rs" "$V/window.rs" | head -60
} 2>&1 | tee /tmp/vendor_diag.txt
echo
echo "✅ Guardado en /tmp/vendor_diag.txt"
