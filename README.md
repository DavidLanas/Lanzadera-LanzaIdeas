# Lanzadera

[[![License: AGPL-3.0-or-later](https://img.shields.io/badge/License-AGPL--3.0--or--later-blue.svg)](https://opensource.org/licenses/AGPL-3.0)
[![Rust](https://img.shields.io/badge/Made%20with-Rust-orange.svg)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/Platform-Linux-lightgrey.svg)](#)
[![GitHub stars](https://img.shields.io/github/stars/DavidLanas/Lanzadera-LanzaIdeas?style=social)](https://github.com/DavidLanas/Lanzadera-LanzaIdeas)
[![GitHub last commit](https://img.shields.io/github/last-commit/DavidLanas/Lanzadera-LanzaIdeas)](https://github.com/DavidLanas/Lanzadera-LanzaIdeas/commits/main)
[![Status](https://img.shields.io/badge/Status-Work%20in%20Progress-blue.svg)](https://github.com/DavidLanas/Lanzadera-LanzaIdeas)

**Mini estación de trabajo creativa con lógica de clip launcher inspirada en Bitwig Studio.**

Lanzadera es una herramienta enfocada en la etapa creativa y la improvisación en vivo.  
Implementa la lógica de **Session View** (clip launching, escenas, cuantización, follow actions, etc.) de forma limpia y modular en Rust.

---

## Características principales

- Lógica de **Session View** estilo Bitwig / Ableton
- Máquina de estados completa de clips (`Empty`, `Triggered`, `Playing`, `Stopping`, `Recording`, `Overdubbing`...)
- Regla de **una voz por pista**
- `LaunchMode`: Trigger, Toggle, Gate, Legato, Repeat
- `FollowAction`: Next, Previous, JumpTo, Random, Again, etc.
- Sistema de cuantización flexible (Immediate, 1/32, 1/16, 1/8, 1/4, 1/2, 1, 2, 4 y 8 barras)
- Arquitectura modular en workspace Rust
- Modelo de datos puro (sin dependencias de audio/UI en el core)

---

## Arquitectura

```text
┌─────────────────────────────────────────────────────────────┐
│                         app / ui                            │
│              (Interfaz de usuario + aplicación)             │
└────────────────────────────┬────────────────────────────────┘
                             │ ControlCmd
┌────────────────────────────▼────────────────────────────────┐
│                       controller                            │
│         (MIDI / hardware → comandos de control)             │
└────────────────────────────┬────────────────────────────────┘
                             │
┌────────────────────────────▼────────────────────────────────┐
│                         core                                │
│   SessionState • Clip • Track • Scene • Quantize • etc.     │
│              (Modelo de datos puro + reductor)              │
└────────────────────────────┬────────────────────────────────┘
                             │
┌────────────────────────────▼────────────────────────────────┐
│                        engine                               │
│              (Motor de audio y reproducción)                │
└─────────────────────────────────────────────────────────────┘

Crates principales
Crate	Responsabilidad
core	Modelo de datos puro, máquina de estados, comandos
engine	Motor de audio, timing, reproducción de clips
controller	Integración con controladores MIDI / hardware
ui	Interfaz de usuario
app	Punto de entrada de la aplicación

El crate core es el corazón del proyecto. Contiene toda la lógica de Session View de forma independiente y reutilizable.
Estructura del repositorio
Texto

Lanzadera-LanzaIdeas/
├── core/                 # Modelo de datos puro
│   └── src/
│       ├── clip.rs       # ClipState, LaunchMode, FollowAction, Quantize
│       ├── session.rs    # SessionState + reductor de comandos
│       ├── track.rs
│       ├── scene.rs
│       └── ...
├── engine/               # Motor de audio
├── controller/           # Controladores MIDI / hardware
├── ui/                   # Interfaz
├── app/                  # Aplicación principal
├── vendor/               # Dependencias parcheadas (vst3-host)
├── Cargo.toml            # Workspace
├── LICENSE
└── README.md

Requisitos

    Rust 1.70 o superior (rustup)
    Linux (desarrollado y probado principalmente en Linux)
    Dependencias del sistema (Debian/Ubuntu):

Bash

sudo apt update
sudo apt install -y build-essential pkg-config libasound2-dev libx11-dev libgl1-mesa-dev

Cómo compilar y ejecutar
Bash

# Clonar el repositorio
git clone https://github.com/DavidLanas/Lanzadera-LanzaIdeas.git
cd Lanzadera-LanzaIdeas

# Compilar en modo release
cargo build --release

# Ejecutar (cuando la aplicación principal esté lista)
cargo run --release -p app

    Nota: El proyecto todavía está en desarrollo activo. Algunas partes de la UI y del motor de audio pueden no estar completamente funcionales.

Estado actual
Componente	Estado
Lógica de Session View	Avanzada
Máquina de estados	Completa
Modelo de datos (core)	Estable
Motor de audio	En desarrollo
UI	En desarrollo
Integración MIDI	En desarrollo
Licencia

Este proyecto está bajo la licencia MIT.

Puedes usarlo, modificarlo y distribuirlo libremente (incluyendo uso comercial y software cerrado), siempre y cuando mantengas el aviso de copyright y la licencia.

Ver el archivo LICENSE [blocked] para más detalles.
Contribuciones

¡Las contribuciones son bienvenidas!

Áreas especialmente interesantes:

    Mejoras en la máquina de estados de clips
    Integración de controladores MIDI (Maschine, Arturia, etc.)
    Motor de audio y timing sample-accurate
    Interfaz de usuario
    Documentación y ejemplos

Abre un Issue o un Pull Request.
Autor

Desarrollado por [David Lanas](https://github.com/DavidLanas)

## Licencia

Lanzadera es software libre: puedes redistribuirlo y/o modificarlo bajo los
términos de la **GNU Affero General Public License v3.0 o posterior**
(`AGPL-3.0-or-later`). Ver el archivo [LICENSE](LICENSE).

Copyright (C) 2026 David Lanas.

This program is free software under the GNU AGPLv3+. If you distribute it or
offer it as a network service, you must release your complete source code
under the same license.

### Componentes de terceros
- `vendor/vst3-host` — ver su propia licencia.
- Symphonia (MPL-2.0), egui/eframe, cpal, midir, arc_swap, hound, ringbuf —
  licencias permisivas compatibles con AGPLv3.
