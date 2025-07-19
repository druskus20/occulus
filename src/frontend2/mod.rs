use tiles::Tabs;

use crate::{TokioEguiBridge, backend::TopLevelBackendEvent, frontend::UiEvent, prelude::*};

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

pub fn run_egui(
    to_backend: tokio::sync::mpsc::UnboundedSender<TopLevelFrontendEvent>,
    from_backend: tokio::sync::mpsc::UnboundedReceiver<TopLevelBackendEvent>,
    tokio_egui_bridge: TokioEguiBridge,
) -> Result<()> {
    eframe::run_native(
        "Oculus",
        eframe::NativeOptions::default(),
        Box::new(|cc| {
            Ok(Box::new(EguiApp {
                tokio_bridge: tokio_egui_bridge,
                to_backend,
                from_backend,
                tabs: Tabs::new(),
            }))
        }),
    )
    .expect("Failed to launch eframe app");
    Ok(())
}

struct EguiApp {
    tokio_bridge: TokioEguiBridge,

    /// Shared data and channels for communication with the backend
    to_backend: tokio::sync::mpsc::UnboundedSender<TopLevelFrontendEvent>,

    from_backend: tokio::sync::mpsc::UnboundedReceiver<TopLevelBackendEvent>,

    tabs: Tabs,
}
#[derive(Debug)]
pub enum TopLevelFrontendEvent {
    OpenStream { on_pane_id: usize },
    CloseStream { on_pane_id: usize },
}

/// Controls different actions throught the rendering of one frame.
/// This is used to bubble up UI interactions
pub struct FrameState {
    add_child_to: Option<egui_tiles::TileId>,
}

impl EguiApp {
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
