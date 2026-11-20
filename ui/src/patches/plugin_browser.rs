//! Browser de plugins con búsqueda fuzzy, pestañas y escaneo en hilo aparte.
use egui::{Key, Ui};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginFormat { Clap, Vst3, Lv2 }

impl PluginFormat {
    pub fn label(self) -> &'static str {
        match self { PluginFormat::Clap => "CLAP",
                     PluginFormat::Vst3 => "VST3",
                     PluginFormat::Lv2 => "LV2" }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginEntry {
    pub name: String, pub vendor: String,
    pub path: PathBuf, pub format: PluginFormat,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PluginCache {
    pub clap: Vec<PluginEntry>, pub vst3: Vec<PluginEntry>, pub lv2: Vec<PluginEntry>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct BrowserPrefs { pub favorites: Vec<String>, pub recent: Vec<String> }

pub struct PluginBrowser {
    pub open: bool, pub search: String, pub tab: PluginFormat,
    pub selected: usize, pub cache: PluginCache, pub prefs: BrowserPrefs,
    pub scanning: Arc<AtomicBool>, pub request_focus: bool,
}

impl Default for PluginBrowser {
    fn default() -> Self {
        Self { open: false, search: String::new(), tab: PluginFormat::Clap,
            selected: 0, cache: PluginCache::default(), prefs: BrowserPrefs::default(),
            scanning: Arc::new(AtomicBool::new(false)), request_focus: false }
    }
}

fn strip_accents(s: &str) -> String {
    s.chars().map(|c| match c {
        'á'|'à'|'ä'|'â'|'ã'|'Á'|'À'|'Ä'|'Â'|'Ã' => 'a',
        'é'|'è'|'ë'|'ê'|'É'|'È'|'Ë'|'Ê' => 'e',
        'í'|'ì'|'ï'|'î'|'Í'|'Ì'|'Ï'|'Î' => 'i',
        'ó'|'ò'|'ö'|'ô'|'õ'|'Ó'|'Ò'|'Ö'|'Ô'|'Õ' => 'o',
        'ú'|'ù'|'ü'|'û'|'Ú'|'Ù'|'Ü'|'Û' => 'u',
        'ñ'|'Ñ' => 'n', 'ç'|'Ç' => 'c', o => o }).collect()
}

pub fn fuzzy_match(query: &str, name: &str) -> bool {
    if query.is_empty() { return true; }
    let q = strip_accents(&query.to_lowercase());
    let n = strip_accents(&name.to_lowercase());
    if n.contains(&q) { return true; }
    let mut it = n.chars();
    q.chars().all(|qc| it.any(|nc| nc == qc))
}

impl PluginBrowser {
    pub fn show(&mut self, ui: &mut Ui) -> Option<PluginEntry> {
        if !self.open { return None; }
        let mut chosen = None;
        egui::Window::new("Plugin Browser").open(&mut self.open)
            .default_size([520.0, 420.0])
            .show(ui.ctx(), |ui| {
                ui.horizontal(|ui| {
                    for f in [PluginFormat::Clap, PluginFormat::Vst3] {
                        if ui.selectable_label(self.tab == f, f.label()).clicked() {
                            self.tab = f; self.selected = 0;
                        }
                    }
                    if ui.button("⟳").clicked() {
                        self.scanning.store(true, Ordering::SeqCst);
                    }
                    if self.scanning.load(Ordering::SeqCst) { ui.spinner(); }
                });
                let sid = ui.make_persistent_id("plsearch");
                let r = ui.add(egui::TextEdit::singleline(&mut self.search)
                    .id(sid).hint_text("Buscar plugin…"));
                if self.request_focus { r.request_focus(); self.request_focus = false; }

                let mut list: Vec<&PluginEntry> = match self.tab {
                    PluginFormat::Clap => self.cache.clap.iter().collect(),
                    PluginFormat::Vst3 => self.cache.vst3.iter().collect(),
                    PluginFormat::Lv2 => self.cache.lv2.iter().collect(),
                };
                list.retain(|p| fuzzy_match(&self.search, &p.name));
                list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

                let mut enter = false;
                let mut esc = false;
                ui.input(|i| {
                    if i.key_pressed(Key::ArrowDown) {
                        self.selected = (self.selected + 1).min(list.len().saturating_sub(1));
                    }
                    if i.key_pressed(Key::ArrowUp) {
                        self.selected = self.selected.saturating_sub(1);
                    }
                    enter = i.key_pressed(Key::Enter);
                    esc = i.key_pressed(Key::Escape);
                });
                if esc { self.open = false; }

                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (i, p) in list.iter().enumerate() {
                        let lbl = format!("{}  ·  {}", p.name, p.vendor);
                        let r = ui.selectable_label(i == self.selected, lbl);
                        if r.clicked() { self.selected = i; }
                        if r.double_clicked() { chosen = Some((*p).clone()); }
                    }
                });
                if enter {
                    if let Some(p) = list.get(self.selected) { chosen = Some((*p).clone()); }
                }
            });
        chosen
    }
}
