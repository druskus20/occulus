use tiles::Tabs;
use tokio::sync::mpsc::unbounded_channel;
use triple_buffer::triple_buffer;

use crate::{
    FrontendSide, TokioEguiBridge,
    backend::{DataToDisplay, LogCounts},
    frontend::{Level, UiEvent},
    prelude::*,
};

mod tiles;

pub(super) mod colors {
    use egui::Color32;
    const COLOR_ERROR: Color32 = Color32::from_rgb(255, 85, 85); // soft red
    const COLOR_WARNING: Color32 = Color32::from_rgb(255, 204, 0); // amber
    const COLOR_INFO: Color32 = Color32::from_rgb(80, 250, 123); // neon green
    const COLOR_DEBUG: Color32 = Color32::from_rgb(139, 233, 253); // cyan
    const COLOR_TRACE: Color32 = Color32::from_rgb(189, 147, 249); // light purple

    const _COLOR_LIHT_PURPLE: Color32 = Color32::from_rgb(189, 147, 249); // light purple
    const COLOR_LIGHT_MAGENTA: Color32 = Color32::from_rgb(255, 121, 198); // light magenta

    const _COLOR_TEXT: Color32 = Color32::from_rgb(255, 255, 255); // white text on dark backgrounds
    const COLOR_TEXT_INV: Color32 = Color32::from_rgb(0, 0, 0); // black text on colored backgrounds
}

pub fn run_egui(frontend_side: FrontendSide, tokio_egui_bridge: TokioEguiBridge) -> Result<()> {
    eframe::run_native(
        "Tracing Log Viewer",
        eframe::NativeOptions::default(),
        Box::new(|cc| Ok(Box::new(EguiApp::init(frontend_side, tokio_egui_bridge)))),
    )
    .expect("Failed to launch eframe app");
    Ok(())
}
impl FrontendSide {}

struct EguiApp {
    tokio_bridge: TokioEguiBridge,

    /// Shared data and channels for communication with the backend
    frontend_side: FrontendSide,

    tabs: Tabs,
}

/// Controls different actions throught the rendering of one frame.
/// This is used to bubble up UI interactions
pub struct FrameState {
    add_child_to: Option<egui_tiles::TileId>,
}

impl EguiApp {
    fn init(frontend_side: FrontendSide, tokio_bridge: TokioEguiBridge) -> Self {
        Self {
            frontend_side,
            tokio_bridge,
            tabs: Tabs::new(),
        }
    }

    /// Acts on a framestate after the UI has been rendered.
    fn process_framestate(&mut self, frame_state: FrameState) {
        if let Some(tile_id) = frame_state.add_child_to {
            self.tabs.add_new_pane_to(tile_id);

            todo!()
            // TODO
            //self.frontend_side.to_backend.send(
            //    BackendMessage::StartNewStream(tile_id),
            //).expect("Failed to send AddNewPaneTo message to backend");
        }
    }
}

impl eframe::App for EguiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut frame_state = FrameState { add_child_to: None };
        egui::CentralPanel::default().show(ctx, |ui| {
            self.tabs.ui(ui, &mut frame_state);
        });
        self.process_framestate(frame_state);
    }
}

struct BakcendEvent {}

struct FrontendBackendComm;

struct ToBackendCommForStream<'a> {
    pub stream_id: usize,
    pub data_buffer_rx: triple_buffer::Output<DataToDisplay>,
    pub to_backend: tokio::sync::mpsc::UnboundedSender<UiEvent>,
    pub from_backend: tokio::sync::mpsc::UnboundedSender<BakcendEvent>,
    pub settings: DisplaySettings<'a>,
}
struct ToFrontendCommForStream<'a> {
    pub stream_id: usize,
    pub data_buffer_tx: triple_buffer::Input<DataToDisplay>,
    pub from_frontend: tokio::sync::mpsc::UnboundedReceiver<UiEvent>,
    pub to_frontend: tokio::sync::mpsc::UnboundedReceiver<BakcendEvent>,
    pub settings: DisplaySettings<'a>,
}

#[derive(Debug, Clone, Copy)]
pub struct DisplaySettings<'a> {
    pub search_string: &'a str,
    pub show_timestamps: bool,
    pub show_targets: bool,
    pub show_file_info: bool,
    pub show_span_info: bool,
    pub auto_scroll: bool,
    pub level_filter: Level,
    pub wrap: bool,
}

impl Default for DisplaySettings<'_> {
    fn default() -> Self {
        Self {
            search_string: "",
            show_timestamps: true,
            show_targets: true,
            show_file_info: true,
            show_span_info: true,
            auto_scroll: true,
            level_filter: Level::Trace,
            wrap: false,
        }
    }
}
struct Logs {}

pub struct StreamData {
    pub filtered_logs: Logs,
    pub log_counts: LogCounts,
}

impl<'a> FrontendBackendComm {
    fn for_stream(stream_id: usize) -> (ToBackendCommForStream<'a>, ToFrontendCommForStream<'a>) {
        let (data_buffer_tx, data_buffer_rx) = triple_buffer(&DataToDisplay::default());
        let (to_backend, from_frontend) = unbounded_channel::<UiEvent>();
        let (from_backend, to_frontend) = unbounded_channel::<BakcendEvent>();
        let settings = DisplaySettings::default();

        (
            ToBackendCommForStream {
                stream_id,
                data_buffer_rx,
                to_backend,
                from_backend,
                settings,
            },
            ToFrontendCommForStream {
                stream_id,
                data_buffer_tx,
                from_frontend,
                to_frontend,
                settings,
            },
        )
    }
}
