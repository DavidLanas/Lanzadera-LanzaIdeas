use egui::Color32;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ClipStateView { #[default] Empty, Triggered, Playing, Stopping, Recording, Overdubbing, RecQueued }
#[derive(Clone, Debug)]
pub struct ClipSnapshot {
    pub name: String, pub color: Color32, pub state: ClipStateView,
    pub progress: f32, pub loop_bars: u8, pub quantize_bars: u8,
    pub peaks: Vec<f32>, pub follow_icon: Option<&'static str>,
}
impl Default for ClipSnapshot {
    fn default() -> Self { Self { name: String::new(), color: Color32::TRANSPARENT, state: ClipStateView::Empty, progress: 0.0, loop_bars: 0, quantize_bars: 0, peaks: Vec::new(), follow_icon: None } }
}
#[derive(Clone, Debug)]
pub struct TrackSnapshot {
    pub name: String, pub color: Color32, pub armed: bool, pub muted: bool,
    pub soloed: bool, pub meter_level: f32, pub clips: Vec<Option<ClipSnapshot>>,
}
#[derive(Clone, Debug, Default)]
pub struct TransportSnapshot {
    pub bar_beat: (u32, u32), pub beat_in_bar: u32, pub bpm: f64,
    pub quantize_bars: u8, pub rec_queued: bool, pub metronome: bool, pub playing: bool,
}
#[derive(Clone, Debug, Default)]
pub struct SessionSnapshot {
    pub tracks: Vec<TrackSnapshot>, pub scene_count: usize, pub transport: TransportSnapshot,
}
