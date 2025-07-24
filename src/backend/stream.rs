// the backend needs to be aware of which tcp tasks are active.
// the data task should be paused if the tcp task is not active.
// connecting the two directly is bad, beucase it removes visibility from the frontend and the
// backend. Instead use the backend as intermediary.

use crate::async_rt::TokioEguiBridge;
use argus::tracing::oculus::DashboardEvent;
use egui::ahash::HashMap;
use egui::mutex::Mutex;
use egui_tiles::TileId;
use futures::{FutureExt, StreamExt};
use std::collections::VecDeque;
use std::error::Error;
use std::panic::catch_unwind;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use std::{env, mem};
use sysinfo::System;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use crate::backend::LogCounts;
use crate::backend::contains_case_insensitive;
use crate::data::StreamData;
use crate::frontend::ScopedUIEvent;
use crate::{backend::LogAppendBuf, data::BackendCommForStream, prelude::*};

use super::{LogAppendBufReader, LogAppendBufWriter, LogCollection};

#[derive(Debug)]
pub struct StreamError {
    pub stream_id: usize,
    pub kind: StreamErrorKind,
}

#[derive(Debug)]
pub enum StreamErrorKind {
    TcpTaskFailed,
    DataTaskFailed,
}

impl std::error::Error for StreamError {}
impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StreamError (ID: {}): {:?}", self.stream_id, self.kind)
    }
}

/// A single stream of logs, which consists in two tasks, a data task and a tcp task.
pub struct Stream {
    stream_id: usize,
    tcp_stream: tokio::net::TcpStream,
    pane_id: TileId,
    done_tcp: bool,

    cancel_data: CancellationToken,
    cancel_tcp: CancellationToken,
    tokio_egui_bridge: TokioEguiBridge,

    comm_with_frontend: BackendCommForStream,
}

pub enum TcpTaskCtrl {}

pub struct StreamHandle {
    stream_id: usize,
    pane_id: TileId,
    cancel_data: CancellationToken,
    cancel_tcp: CancellationToken,

    tcp_task: Option<tokio::task::JoinHandle<std::result::Result<(), StreamError>>>,
    data_task: tokio::task::JoinHandle<std::result::Result<(), StreamError>>,

    tokio_egui_bridge: TokioEguiBridge,
}

impl StreamHandle {
    #[tracing::instrument(skip_all)]
    pub fn terminate(&self) {
        trace!("Terminating stream {}", self.stream_id);
        self.cancel_tcp.cancel();
        self.cancel_data.cancel();
    }
}
impl Stream {
    pub fn new(
        stream_id: usize,
        tcp_stream: tokio::net::TcpStream,
        pane_id: TileId,
        comm_with_frontend: BackendCommForStream,
        tokio_egui_bridge: TokioEguiBridge,
    ) -> Self {
        Self {
            stream_id,
            tcp_stream,
            pane_id,
            done_tcp: false,
            cancel_data: CancellationToken::new(),
            cancel_tcp: CancellationToken::new(),
            tokio_egui_bridge,
            comm_with_frontend,
        }
    }

    pub async fn run(mut self) -> std::result::Result<(), StreamError> {
        let egui_ctx = self.tokio_egui_bridge.wait_egui_ctx().await;

        debug!("Starting new stream with ID {}", self.stream_id);

        // Communication between tasks
        let (incoming_logs_tx, incoming_logs_rx) = LogAppendBuf::split(); // logs
        //let (to_data_ctrl, from_tcp_ctrl) = unbounded_channel::<DataTaskCtrl>(); // ctrl // TODO:
        // this probably goes away

        // TCP TASK
        let cancel_tcp = CancellationToken::new();
        let tcp_task = TcpTask::new(
            self.stream_id,
            self.tcp_stream,
            //to_data_ctrl,
            cancel_tcp.clone(),
            incoming_logs_tx,
        )
        .spawn()
        .fuse();

        // DATA TASK
        let cancel_data = CancellationToken::new();
        let data_task = DataTask::new(
            self.comm_with_frontend,
            egui_ctx,
            incoming_logs_rx,
            cancel_data.clone(),
        )
        .spawn()
        .fuse();

        tokio::pin!(tcp_task);
        tokio::pin!(data_task);
        //futures::pin_mut!(tcp_task);
        //futures::pin_mut!(data_task);

        let mut tcp_done = false;
        let mut data_done = false;
        loop {
            tokio::select! {
                r = &mut data_task, if !data_done => {
                    data_done = true;
                    match r {
                        Ok(Ok(())) => debug!("Data task completed successfully"),
                        Ok(Err(e)) => error!("Data task encountered an error"),
                        Err(e) => error!("Data task panicked: {:?}", e),
                    }
                }

                r = &mut tcp_task, if !tcp_done => {
                    self.done_tcp = true;
                    tcp_done = true;
                    match r {
                        Ok(Ok(())) => debug!("TCP task completed successfully"),
                        Ok(Err(e)) => error!("TCP task encountered an error"),
                        Err(e) => error!("TCP task panicked: {:?}", e),
                    }
                }
                else => {
                    // Only return from the steram when both tasks are done
                    break;

                }
            }
        }
        Ok(())
    }
}
pub struct DataTask {
    incoming_logs_buffer: LogAppendBufReader<Arc<DashboardEvent>>,

    comms: BackendCommForStream,

    /// All logs, kept in memory for the lifetime of the task
    all_logs: LogCollection,
    /// Filtered logs, matching the current filter. These are the logs that will be displayed in
    /// the UI.
    ///
    /// TODO: this might be redundant with the display_data_tx (in backend)
    filtered_logs: LogCollection,

    egui_ctx: egui::Context,

    #[allow(unused)]
    cancel: CancellationToken,
}

pub struct TcpTask {
    incoming_logs_writer: LogAppendBufWriter<Arc<DashboardEvent>>,
    stream_id: usize,
    tcp_stream: TcpStream,

    #[allow(unused)]
    cancel: CancellationToken,
}

pub enum DataTaskCtrl {
    StartTimer,
    StopTimer,
}

impl TcpTask {
    pub fn new(
        stream_id: usize,
        tcp_stream: TcpStream,
        cancel: CancellationToken,
        incoming_logs_writer: LogAppendBufWriter<Arc<DashboardEvent>>,
    ) -> Self {
        Self {
            cancel,
            tcp_stream,
            incoming_logs_writer,
            stream_id,
        }
    }

    //pub fn spawn_on(self, task_set: &mut JoinSet<std::result::Result<(), StreamError>>) {
    pub fn spawn(self) -> JoinHandle<std::result::Result<(), StreamError>> {
        //task_set.spawn(async move {
        tokio::task::spawn(async move {
            let stream_id = self.stream_id;
            debug!("Starting TCP task");
            self.tcp_loop().await.map_err(|e| {
                error!("TCP task failed: {:?}", e);
                StreamError {
                    stream_id,
                    kind: StreamErrorKind::TcpTaskFailed,
                }
            })?;
            Ok(())
        })
    }

    async fn tcp_loop(self) -> Result<()> {
        // Forward messages
        self.handle_log_stream()
            .await
            .wrap_err(format!("Connection handling failed"))?;

        Ok(())
    }

    async fn handle_log_stream(self) -> Result<()> {
        let reader = BufReader::new(self.tcp_stream);
        let mut lines = reader.lines();

        loop {
            tokio::select! {
                result = lines.next_line() => {
                    match result {
                        Ok(Some(line)) => {
                            let event = serde_json::from_str::<DashboardEvent>(&line)?;
                            self.incoming_logs_writer.push(Arc::new(event));
                        }
                        Ok(None) => break, // EOF
                        Err(e) => return Err(e.into()),
                    }
                }
                _ = self.cancel.cancelled() => {
                    info!("Log stream cancelled.");
                    break;
                }
            }
        }
        Ok(())
    }

    fn cancel(&self) {
        debug!("Cancelling TCP task");
        self.cancel.cancel();
    }
}

impl DataTask {
    pub fn new(
        comms: BackendCommForStream,
        egui_ctx: egui::Context,
        incoming_logs_buffer: LogAppendBufReader<Arc<DashboardEvent>>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            all_logs: VecDeque::new(),
            filtered_logs: VecDeque::new(),
            incoming_logs_buffer,
            egui_ctx,
            cancel,
            comms,
        }
    }

    /// Starts running once a `DataTaskCtrl::Start` message is received. (by the TCP task)
    //pub fn spawn_on(mut self, task_set: &mut JoinSet<std::result::Result<(), StreamError>>) {
    pub fn spawn(mut self) -> JoinHandle<std::result::Result<(), StreamError>> {
        const TICK_INTERVAL_MS: u64 = 100;
        let mut timer = tokio::time::interval(Duration::from_millis(1000));
        let mut timer_enabled = true; // Start with timer enabled

        tokio::task::spawn(async move {
            //task_set.spawn(async move {
            debug!("Starting DataTask");
            loop {
                tokio::select! {
                    r = self.comms.from_frontend.recv() => {
                        match r {
                            Some(event) => {
                                trace!("Received UI event: {:?}", event);
                                self.handle_ui_event(event).await;
                            }
                            None => {
                                return Ok(());
                            }
                        }
                    },
                    _ = timer.tick(), if timer_enabled => {
                        if self.incoming_logs_buffer.dropped_writer {
                            warn!("Incoming logs writer was dropped, stopping timer");
                            timer_enabled = false; // Stop the timer
                        }
                        match self.publish_new_logs().await {
                            Ok(_) => trace!("Published new logs to EGUI context"),
                            Err(e) => {
                                error!("Failed to publish new logs: {:?}", e);
                            }
                        }
                    }
                };
            }
        })
    }

    // publush new logs to the EGUI context
    #[tracing::instrument(skip_all)]
    pub async fn publish_new_logs(&mut self) -> Result<()> {
        trace!("Publishing new logs to EGUI context");
        let log_display_settings = self.comms.settings.clone();

        // Process new logs from the tcp task
        // order here avoids unnecessary clone
        //
        // BUG: the egui shared buffer cannot be extended because internally there are
        // more than one buffer, so we need to swap the buffer and then extend it
        let new_logs = self.incoming_logs_buffer.swap();

        // Pre-compute the layout jobs for the new logs
        let filtered_new_logs = new_logs
            .iter()
            // Filter with the current log display settings
            .filter(|event| {
                (event.level >= log_display_settings.level_filter.into())
                    && if !self.comms.settings.search_string.is_empty() {
                        contains_case_insensitive(
                            event.message.as_str(),
                            self.comms.settings.search_string.as_str(),
                        )
                    } else {
                        true
                    }
            })
            .cloned()
            .collect::<Vec<_>>();

        self.all_logs.extend(new_logs);

        // This avoids swapping the buffers unnecessarily
        if filtered_new_logs.is_empty() {
            trace!("No new logs to publish (matching the current filter)");
            return Ok(());
        }

        // Update and publish the new logs to the EGUI context
        info!(
            "Requesting EGUI repaint with {} new jobs",
            filtered_new_logs.len()
        );
        self.filtered_logs.extend(filtered_new_logs);

        // All the logs - we cannot extend, becasue how triple_buffer is implemented
        *self.comms.data_buffer_tx.input_buffer_mut() = StreamData {
            filtered_logs: self.filtered_logs.clone(),
            log_counts: LogCounts::from_logs(&self.all_logs),
        };
        self.comms.data_buffer_tx.publish();
        self.egui_ctx.request_repaint();

        Ok(())
    }

    async fn handle_ui_event(&mut self, event: ScopedUIEvent) {
        fn open_in_nvim(file_path: &str, line: u32) {
            info!("Opening file {} at line {}", file_path, line);
            // Check that $EDITOR is set to nvim
            let editor = env::var("EDITOR").unwrap_or_else(|_| "nvim".to_string());
            if !editor.ends_with("nvim") {
                error!("$EDITOR is not set to nvim, cannot open file in Neovim");
                return;
            }
            if !std::path::Path::new("/tmp/nvim.sock").exists() {
                error!(
                    "/tmp/nvim.sock does not exists, cannot open file in Neovim.\n
                    Try opening Neovim or running: :call serverstart('/tmp/nvim.sock')"
                );
                return;
            }
            // Check that nvim is already running and listening
            if Command::new("nvim")
                .arg("--server")
                .arg("/tmp/nvim.sock")
                .arg("--remote-expr")
                .arg("v:servername")
                .status()
                .is_err()
            {
                error!("Neovim is not running, cannot open file in Neovim");
                return;
            }

            let cmd = format!("<Cmd>edit {file_path}<CR>{line}G");
            let status = Command::new("nvim")
                .args(["--server", "/tmp/nvim.sock", "--remote-send", &cmd])
                .status();

            if let Err(e) = status {
                error!("Failed to open file {file_path}:{line} in Neovim: {e}");
            }
        }

        match event {
            ScopedUIEvent::LogDisplaySettingsChanged(new_settings) => {
                info!("Received new log display settings: {:?}", new_settings);
                self.comms.settings = new_settings;
                self.filter_and_refresh_egui_buf().await;
            }
            ScopedUIEvent::OpenInEditor { path, line } => {
                // verify that the file exists
                let path = std::path::PathBuf::from(path);
                if !path.exists() {
                    error!("File does not exist: {}", path.display());
                    return;
                }
                open_in_nvim(&path.to_string_lossy(), line);
            }
            ScopedUIEvent::Clear => {
                info!("Clearing all logs");
                self.all_logs.clear();
                self.filtered_logs.clear();
                // Reset the data buffer
                self.comms
                    .data_buffer_tx
                    .input_buffer_mut()
                    .filtered_logs
                    .clear();
                self.comms
                    .data_buffer_tx
                    .input_buffer_mut()
                    .log_counts
                    .reset();

                self.comms.data_buffer_tx.publish();
                self.egui_ctx.request_repaint();
            }
        }
    }

    async fn filter_and_refresh_egui_buf(&mut self) {
        trace!("Refreshing EGUI buffer");
        let all_logs = &self.all_logs;

        let log_counts = LogCounts::from_logs(all_logs);
        let filtered_logs = all_logs
            .iter()
            .filter(|event| {
                (event.level >= self.comms.settings.level_filter.into())
                    && if !self.comms.settings.search_string.is_empty() {
                        contains_case_insensitive(
                            event.message.as_str(),
                            self.comms.settings.search_string.as_str(),
                        )
                    } else {
                        true
                    }
            })
            .cloned()
            .collect::<VecDeque<_>>();

        *self.comms.data_buffer_tx.input_buffer_mut() = StreamData {
            filtered_logs,
            log_counts,
        };
        // Refresh the triple buffer to reflect the new settings
        self.comms.data_buffer_tx.publish();
        self.egui_ctx.request_repaint();
    }
}
