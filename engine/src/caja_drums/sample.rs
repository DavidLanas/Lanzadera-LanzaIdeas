//! Muestra decodificada estéreo. Versión mínima del plugin/src/sample.rs
//! sin dependencias de nih-plug (solo Arc<[f32]>).

pub struct Sample {
    pub left: Vec<f32>,
    pub right: Option<Vec<f32>>,
}

impl Sample {
    pub fn is_empty(&self) -> bool { self.left.is_empty() }
    pub fn len(&self) -> usize { self.left.len() }
}
