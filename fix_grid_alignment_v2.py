import os

if not os.path.isdir("ui/src"):
    print("❌ Ejecuta esto desde la raíz de Lanzadera-LanzaIdeas")
    exit(1)

filepath = "ui/src/patches/clip_grid.rs"

with open(filepath, "r", encoding="utf-8") as f:
    content = f.read()

# Usar allocate_exact_size con Sense::hover() para que la grilla mida la celda correctamente
content = content.replace(
    "ui.dummy(vec2(header_w, scene_header_h));",
    "ui.allocate_exact_size(vec2(header_w, scene_header_h), Sense::hover());"
)

with open(filepath, "w", encoding="utf-8") as f:
    f.write(content)

print("✅ Alineación corregida usando allocate_exact_size (compatible con egui 0.28)")
