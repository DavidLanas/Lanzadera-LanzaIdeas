#!/usr/bin/env bash
echo "=== Sesión gráfica ==="
echo "XDG_SESSION_TYPE=$XDG_SESSION_TYPE  DISPLAY=$DISPLAY  WAYLAND_DISPLAY=$WAYLAND_DISPLAY"
echo
echo "=== Wine / yabridge ==="
wine --version 2>/dev/null || echo "wine no encontrado en PATH"
command -v yabridgectl >/dev/null && yabridgectl status || echo "yabridgectl no encontrado"
echo
echo "=== Plugins yabridge detectados ==="
find "$HOME/.vst3" "$HOME/.clap" "$HOME/.vst" -iname '*yabridge*' 2>/dev/null | head -40
echo
echo "=== Ahora abre Lanzadera, carga un plugin yabridge que falle ==="
echo "=== y copia aquí la línea que empiece por '⚠ VST3 pista' o '[gui]' ==="
