// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! Serialización JSON del estado (patrón de persist de Caja).
//! El archivo de proyecto .lanzadera es este JSON, con `format_version`.
use crate::session::{SessionState, FORMAT_VERSION};

impl SessionState {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    pub fn from_json(s: &str) -> Result<SessionState, String> {
        let mut st: SessionState = serde_json::from_str(s).map_err(|e| e.to_string())?;
        st.migrate()?;
        Ok(st)
    }

    /// Lleva un proyecto de una versión anterior al formato actual. Un proyecto de una
    /// versión MÁS NUEVA se rechaza (abrirlo y guardarlo perdería datos que no entendemos).
    pub fn migrate(&mut self) -> Result<(), String> {
        if self.format_version > FORMAT_VERSION {
            return Err(format!(
                "El proyecto es del formato {} y esta versión de Lanzadera solo entiende hasta el {}. Actualiza Lanzadera.",
                self.format_version, FORMAT_VERSION
            ));
        }
        // v0/v1 → v2: sin cambios de estructura (los campos nuevos usan `serde(default)`).
        // Futuras versiones se encadenan aquí: `if self.format_version < 3 { ... }`
        self.format_version = FORMAT_VERSION;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_json_value() -> serde_json::Value {
        serde_json::from_str(&SessionState::default().to_json()).unwrap()
    }

    #[test]
    fn proyecto_sin_version_se_migra() {
        let mut v = default_json_value();
        v.as_object_mut().unwrap().remove("format_version");
        let s = SessionState::from_json(&v.to_string()).unwrap();
        assert_eq!(s.format_version, FORMAT_VERSION);
    }

    #[test]
    fn proyecto_de_version_futura_da_error() {
        let mut v = default_json_value();
        v["format_version"] = serde_json::json!(FORMAT_VERSION + 1);
        assert!(SessionState::from_json(&v.to_string()).is_err());
    }

    #[test]
    fn ida_y_vuelta_conserva_la_version() {
        let s = SessionState::from_json(&SessionState::default().to_json()).unwrap();
        assert_eq!(s.format_version, FORMAT_VERSION);
    }
}
