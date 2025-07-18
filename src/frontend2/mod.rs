use tiles::TabbedLayout;

use crate::{FrontendSide, TokioEguiBridge, prelude::*};

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

    tabbed_layout: TabbedLayout,
}

impl EguiApp {
    fn init(frontend_side: FrontendSide, tokio_bridge: TokioEguiBridge) -> Self {
        Self {
            frontend_side,
            tokio_bridge,
            tabbed_layout: TabbedLayout::new(),
        }
    }
}

impl eframe::App for EguiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label("Hello, Egui!");
            self.tabbed_layout.ui(ui);
        });
    }
}
