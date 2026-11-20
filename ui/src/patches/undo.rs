use std::collections::VecDeque;

pub trait Command {
    fn apply(&mut self);
    fn undo(&mut self);
    fn name(&self) -> &str;
}

pub struct UndoStack {
    undo: VecDeque<Box<dyn Command>>,
    redo: Vec<Box<dyn Command>>,
    max: usize,
}

impl UndoStack {
    pub fn new(max: usize) -> Self {
        Self { undo: VecDeque::with_capacity(max), redo: Vec::new(), max }
    }
    pub fn push(&mut self, mut c: Box<dyn Command>) {
        c.apply();
        if self.undo.len() >= self.max { self.undo.pop_front(); }
        self.undo.push_back(c);
        self.redo.clear();
    }
    pub fn undo(&mut self) -> bool {
        if let Some(mut c) = self.undo.pop_back() {
            c.undo(); self.redo.push(c); true
        } else { false }
    }
    pub fn redo(&mut self) -> bool {
        if let Some(mut c) = self.redo.pop() {
            c.apply(); self.undo.push_back(c); true
        } else { false }
    }
    pub fn can_undo(&self) -> bool { !self.undo.is_empty() }
    pub fn can_redo(&self) -> bool { !self.redo.is_empty() }
}
