//! Serialización JSON del estado (patrón de persist de Caja).
//! El archivo de proyecto .lanzadera es este JSON (Fase 2: diálogo de guardado).
use crate::session::SessionState;

impl SessionState {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }
    pub fn from_json(s: &str) -> Result<SessionState, String> {
        serde_json::from_str(s).map_err(|e| e.to_string())
    }
}
