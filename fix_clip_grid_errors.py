import os

if not os.path.isdir("ui/src"):
    print("❌ Ejecuta esto desde la raíz de Lanzadera-LanzaIdeas")
    exit(1)

filepath = "ui/src/patches/clip_grid.rs"

with open(filepath, "r", encoding="utf-8") as f:
    content = f.read()

# 1. Agregar vec2 a los imports de egui
content = content.replace(
    "use egui::{Align2, Button, Color32, FontId, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, Ui, Vec2};",
    "use egui::{vec2, Align2, Button, Color32, FontId, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, Ui, Vec2};"
)

# 2. Corregir el Option<&ClipSnapshot> (quitar el .as_ref() innecesario)
content = content.replace(
    "let clip = track.clips.get(si).and_then(|c| c.as_ref());",
    "let clip = track.clips.get(si);"
)

with open(filepath, "w", encoding="utf-8") as f:
    f.write(content)

print("✅ Errores de vec2 y as_ref corregidos en clip_grid.rs")
