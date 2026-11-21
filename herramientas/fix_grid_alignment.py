import os

if not os.path.isdir("ui/src"):
    print("❌ Ejecuta esto desde la raíz de Lanzadera-LanzaIdeas")
    exit(1)

filepath = "ui/src/patches/clip_grid.rs"

with open(filepath, "r", encoding="utf-8") as f:
    content = f.read()

# Reemplazar allocate_space por dummy para que egui::Grid lo reconozca como una celda válida
# y alinee perfectamente la columna de cabeceras de pista con las celdas de clip.
content = content.replace(
    "ui.allocate_space(vec2(header_w, scene_header_h));",
    "ui.dummy(vec2(header_w, scene_header_h));"
)

with open(filepath, "w", encoding="utf-8") as f:
    f.write(content)

print("✅ Alineación de grilla corregida (ui.dummy asegura que la Escena 1 quede justo encima de la Pista 1)")
