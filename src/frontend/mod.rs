use crate::{
    TokioEguiBridge,
    backend::TopLevelBackendEvent,
    data::{DisplaySettings, FrontendCommForStream},
    prelude::*,
};
use egui::{Vec2, ahash::HashMap, vec2};
use egui_tiles::TileId;
use pane::Pane;
use tiles::Tabs;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

mod pane;
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

const DEFAULT_SIZE: Vec2 = vec2(1000.0, 1000.0);

pub fn run_egui(
    to_backend: UnboundedSender<TopLevelFrontendEvent>,
    from_backend: UnboundedReceiver<TopLevelBackendEvent>,
    tokio_egui_bridge: TokioEguiBridge,
) -> Result<()> {
    let window_builder = Box::new(|vp: egui::ViewportBuilder| vp.with_inner_size(DEFAULT_SIZE));
    eframe::run_native(
        "Oculus",
        eframe::NativeOptions {
            window_builder: Some(window_builder),
            ..Default::default()
        },
        Box::new(|cc| {
            let ctx = cc.egui_ctx.clone();
            tokio_egui_bridge.register_egui_context(ctx);

            let tabs = Tabs::empty();
            //let tile_id = tabs.add_new_pane_to(tabs.root_tile());
            //to_backend.send(TopLevelFrontendEvent::OpenStream {
            //    on_pane_id: tile_id,
            //})?;

            Ok(Box::new(EguiApp {
                tokio_bridge: tokio_egui_bridge,
                to_backend,
                from_backend,
                tabs,
                streams: HashMap::default(),
            }))
        }),
    )
    .expect("Failed to launch eframe app");
    Ok(())
}

#[derive(Debug)]
struct EguiApp {
    tokio_bridge: TokioEguiBridge,

    to_backend: UnboundedSender<TopLevelFrontendEvent>,
    from_backend: UnboundedReceiver<TopLevelBackendEvent>,

    tabs: Tabs,

    streams: HashMap<usize, StreamMeta>,
}

#[derive(Debug, Clone)]
struct StreamMeta {
    pane_id: Option<TileId>,
    stream_id: usize,
    pending: bool,
}

#[derive(Debug)]
pub enum TopLevelFrontendEvent {
    OpenStream {
        stream_id: usize,
        on_pane_id: TileId,
    },
    CloseStream {
        stream_id: usize,
        on_pane_id: TileId,
    },
}

/// Controls different actions throught the rendering of one frame.
/// This is used to bubble up UI interactions
#[derive(Debug, Default)]
pub struct FrameState {
    open_pending_stream: Option<usize>,
    pane_has_been_closed: Option<egui_tiles::TileId>,
}

impl EguiApp {
    /// Acts on a framestate after the UI has been rendered.
    fn process_framestate(&mut self, frame_state: FrameState) -> Result<()> {
        if let Some(stream_id) = frame_state.open_pending_stream {
            let initial_tile = self
                .streams
                .get(&stream_id)
                .and_then(|meta| meta.pane_id)
                .unwrap_or(self.tabs.root_tile());

            let new_tile_id = self.tabs.add_new_pane_to(self.tabs.root_tile(), stream_id);

            debug!(
                "Adding new pane {:?} to tile: {:?}",
                new_tile_id,
                self.tabs.root_tile()
            );

            self.to_backend.send(TopLevelFrontendEvent::OpenStream {
                on_pane_id: new_tile_id,
                stream_id,
            })?;
        }
        if let Some(tile_id) = frame_state.pane_has_been_closed {
            debug!("Pane has been closed: {:?}", tile_id);

            let stream_id = self.get_pane(tile_id).associated_stream_id;

            self.to_backend.send(TopLevelFrontendEvent::CloseStream {
                on_pane_id: tile_id,
                stream_id,
            })?;
        }
        Ok(())
    }

    fn receive_and_process_backend_events(&mut self) -> Result<()> {
        while let Ok(event) = self.from_backend.try_recv() {
            match event {
                TopLevelBackendEvent::NewPendingStream { stream_id, addr } => {
                    info!(
                        "New pending stream with ID {} at address {}",
                        stream_id, addr
                    );

                    self.streams.insert(
                        stream_id,
                        StreamMeta {
                            pane_id: None,
                            stream_id,
                            pending: true,
                        },
                    );
                }
                TopLevelBackendEvent::StreamStarted(comm) => {
                    debug!(
                        "Stream started with ID {} on pane {:?}",
                        comm.stream_id, comm.pane_id
                    );
                    let stream = self
                        .streams
                        .get_mut(&comm.stream_id)
                        .expect("Stream should be present");

                    stream.pending = false;
                    stream.pane_id = Some(comm.pane_id);

                    let pane = self
                        .tabs
                        .get_pane_with_id_mut(comm.pane_id)
                        .expect("Pane should exist");

                    debug!(
                        "Initializing comm for stream ID {} on pane {:?}",
                        comm.stream_id, comm.pane_id
                    );
                    pane.init_comm(comm);
                }
            }
        }
        Ok(())
    }

    fn get_pane(&self, tile_id: TileId) -> &Pane {
        self.tabs
            .get_pane_with_id(tile_id)
            .expect("Pane should exist")
    }

    fn render_pending_streams_window(&mut self, ctx: &egui::Context, frame_state: &mut FrameState) {
        egui::Window::new("Pending Streams")
            .default_pos(egui::pos2(100.0, 100.0))
            .show(ctx, |ui| {
                for (stream_id, stream_meta) in &self.streams {
                    if stream_meta.pending {
                        ui.horizontal(|ui| {
                            ui.label(format!("Stream ID: {stream_id}"));
                            if ui.button("Open").clicked() {
                                frame_state.open_pending_stream = Some(*stream_id);
                            }
                        });
                    }
                }
            });
    }
}

impl eframe::App for EguiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.receive_and_process_backend_events()
            .expect("Failed to process backend events");

        let mut frame_state = FrameState::default();
        egui::CentralPanel::default().show(ctx, |ui| {
            self.tabs.ui(ui, &mut frame_state);
        });

        self.render_pending_streams_window(ctx, &mut frame_state);

        self.process_framestate(frame_state)
            .expect("Failed to process frame state");
    }
}

#[derive(Debug)]
pub enum ScopedUIEvent {
    LogDisplaySettingsChanged(DisplaySettings),
    OpenInEditor { path: String, line: u32 },
    Clear,
}
