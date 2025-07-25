use std::{collections::VecDeque, sync::Arc};

use argus::tracing::oculus::DashboardEvent;
use egui_tiles::TileId;
use tokio::sync::mpsc::unbounded_channel;
use triple_buffer::triple_buffer;

use crate::{
    backend::{BakcendEvent, LogCounts},
    frontend::ScopedUIEvent,
};

// Frontend and backend communication
// 1) It allows the background tokio tasks to publish data to the GUI
// 2) it allos the GUI to send events to the tokio tasks (i.e. Settings changed, start/stop...)
pub struct FrontendBackendComm;

#[derive(Debug)]
pub struct FrontendCommForStream {
    pub stream_id: usize,
    pub pane_id: egui_tiles::TileId,
    pub data_buffer_rx: triple_buffer::Output<StreamData>,
    pub to_backend: tokio::sync::mpsc::UnboundedSender<ScopedUIEvent>,
    pub from_backend: tokio::sync::mpsc::UnboundedSender<BakcendEvent>,
    pub settings: DisplaySettings,
}

#[derive(Debug)]
pub struct BackendCommForStream {
    pub stream_id: usize,
    pub pane_id: egui_tiles::TileId,
    pub data_buffer_tx: triple_buffer::Input<StreamData>,
    pub from_frontend: tokio::sync::mpsc::UnboundedReceiver<ScopedUIEvent>,
    pub to_frontend: tokio::sync::mpsc::UnboundedReceiver<BakcendEvent>,
    pub settings: DisplaySettings,
}

impl FrontendBackendComm {
    pub fn for_stream(
        stream_id: usize,
        pane_id: TileId,
    ) -> (FrontendCommForStream, BackendCommForStream) {
        let (data_buffer_tx, data_buffer_rx) = triple_buffer(&StreamData::default());
        let (to_backend, from_frontend) = unbounded_channel::<ScopedUIEvent>();
        let (from_backend, to_frontend) = unbounded_channel::<BakcendEvent>();
        let settings = DisplaySettings::default();

        (
            FrontendCommForStream {
                stream_id,
                pane_id,
                data_buffer_rx,
                to_backend,
                from_backend,
                settings: settings.clone(),
            },
            BackendCommForStream {
                stream_id,
                pane_id,
                data_buffer_tx,
                from_frontend,
                to_frontend,
                settings: settings.clone(),
            },
        )
    }
}

type Logs = VecDeque<Arc<DashboardEvent>>;

#[derive(Debug, Clone, Default)]
pub struct StreamData {
    pub filtered_logs: Logs,
    pub log_counts: LogCounts,
}

#[derive(Debug, Clone)]
pub struct DisplaySettings {
    pub search_string: Arc<String>,
    pub show_timestamps: bool,
    pub show_targets: bool,
    pub show_file_info: bool,
    pub show_span_info: bool,
    pub auto_scroll: bool,
    pub level_filter: Level,
    pub wrap: bool,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            search_string: Arc::new(String::new()),
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

#[derive(Debug, Clone, PartialEq, Copy)]
pub enum Level {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl From<argus::tracing::oculus::Level> for Level {
    fn from(val: argus::tracing::oculus::Level) -> Self {
        match val {
            argus::tracing::oculus::Level::TRACE => Level::Trace,
            argus::tracing::oculus::Level::DEBUG => Level::Debug,
            argus::tracing::oculus::Level::INFO => Level::Info,
            argus::tracing::oculus::Level::WARN => Level::Warn,
            argus::tracing::oculus::Level::ERROR => Level::Error,
        }
    }
}

impl From<Level> for argus::tracing::oculus::Level {
    fn from(val: Level) -> Self {
        match val {
            Level::Trace => argus::tracing::oculus::Level::TRACE,
            Level::Debug => argus::tracing::oculus::Level::DEBUG,
            Level::Info => argus::tracing::oculus::Level::INFO,
            Level::Warn => argus::tracing::oculus::Level::WARN,
            Level::Error => argus::tracing::oculus::Level::ERROR,
        }
    }
}
