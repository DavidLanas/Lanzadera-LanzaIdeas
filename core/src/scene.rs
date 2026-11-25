// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! Escena = fila del launcher (lanza todos los clips de la columna).
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Scene {
    pub name: String,
    pub color: [u8; 3],
    pub tempo: Option<f64>, // si None, usa el tempo global
}

impl Default for Scene {
    fn default() -> Self {
        Scene { name: String::new(), color: [150, 150, 150], tempo: None }
    }
}
