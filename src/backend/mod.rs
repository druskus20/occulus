use crate::data::{BackendCommForStream, FrontendBackendComm, FrontendCommForStream, StreamData};
use crate::frontend::{ScopedUIEvent, TopLevelFrontendEvent};
use crate::prelude::*;

use crate::async_rt::TokioEguiBridge;
use argus::tracing::oculus::DashboardEvent;
use egui::ahash::HashMap;
use egui::mutex::Mutex;
use egui_tiles::TileId;
use futures::StreamExt;
use std::collections::VecDeque;
use std::error::Error;
use std::panic::catch_unwind;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use std::{env, mem};
use stream::{Stream, StreamError, StreamErrorKind, StreamHandle};
use sysinfo::System;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

mod stream;

#[derive(Debug)]
pub enum TopLevelBackendEvent {
    NewPendingStream {
        stream_id: usize,
        addr: std::net::SocketAddr,
    },
    StreamStarted(FrontendCommForStream),
}
pub struct BakcendEvent {}

/// Manages multiple streams
pub struct Backend {
    pending_streams: HashMap<usize, TcpStream>,
    streams: HashMap<usize, StreamHandle>,

    stream_task_set: JoinSet<std::result::Result<(), StreamError>>,
    egui_ctx: egui::Context,
    tokio_egui_bridge: TokioEguiBridge,
    last_stream_id: usize,
    to_frontend: UnboundedSender<TopLevelBackendEvent>,
    from_frontend: UnboundedReceiver<TopLevelFrontendEvent>,
}

impl Backend {
    pub async fn init(
        egui_ctx: egui::Context,
        from_frontend: UnboundedReceiver<TopLevelFrontendEvent>,
        to_frontend: UnboundedSender<TopLevelBackendEvent>,
        tokio_egui_bridge: TokioEguiBridge,
    ) -> Self {
        Backend {
            egui_ctx,
            streams: HashMap::default(),
            tokio_egui_bridge: tokio_egui_bridge.clone(),
            last_stream_id: 0,
            to_frontend,
            from_frontend,
            pending_streams: HashMap::default(),
            stream_task_set: JoinSet::new(),
        }
    }
    pub async fn move_to_ephemeral_port(&mut self, mut stream: TcpStream) -> Result<TcpStream> {
        // Create ephemeral listener
        let ephemeral_listener = TcpListener::bind("0.0.0.0:0").await?;
        let ephemeral_port = ephemeral_listener.local_addr()?.port();

        println!("Created ephemeral port: {}", ephemeral_port);

        // Send ephemeral port to client
        let msg = format!("PORT {}\n", ephemeral_port);
        stream.write_all(msg.as_bytes()).await?;

        Ok(stream)
    }

    pub async fn run(&mut self) {
        let address = "127.0.0.1:8080".to_string();
        let listener = TcpListener::bind(&address)
            .await
            .expect("Failed to bind TCP listener");
        info!("TCP server listening on {}", address);

        loop {
            tokio::select! {
                r = listener.accept() => {
                    match r {
                        Ok((tcp_stream, addr)) => {
                            info!("Accepted connection from {}", addr);
                            let tcp_stream = self.move_to_ephemeral_port(tcp_stream).await.expect("Failed to move to ephemeral port");
                            self.last_stream_id += 1;
                            let stream_id = self.last_stream_id;
                            self.pending_streams.insert(stream_id, tcp_stream);
                            self.to_frontend.send(TopLevelBackendEvent::NewPendingStream{ stream_id: stream_id, addr });
                        }
                        Err(e) => {
                            error!("Failed to accept TCP connection: {:?}", e);
                        }
                    }
                },
                Some(join_handle) = self.stream_task_set.join_next() => {
                    match join_handle {
                        Ok(Ok(())) => {
                            info!("Stream task completed successfully");
                        }
                        Ok(Err(e)) => {
                            let stream_id = e.stream_id;
                            error!("Stream task failed with error: {:?}", e);
                            if let Some(stream_handle) = self.streams.remove(&stream_id) {
                                stream_handle.terminate();
                            }
                            else {
                                trace!("Stream with ID {} was not found in the streams map", stream_id);
                                // Stream was already removed, this is fine since the error from
                                // both data and tcp tasks could be the cause
                            }
                        }

                        Err(e) => {
                            error!("Stream task panicked: {:?}", e);
                        }
                    }
                },
                event = self.from_frontend.recv() => {
                    match event {
                        Some(event) => self
                            .handle_top_level_frontend_event(event)
                            .await
                            .expect("Failed to handle frontend event"),
                        None => {
                            info!("Frontend event channel closed, shutting down backend");
                            break;
                        }
                    }
                },
               _ = self.tokio_egui_bridge.cancelled_fut() => {
                    warn!("Tokio task cancelled, shutting down...");
                    break; // important!
                },
            };
        }

        debug!("Backend event loop exited");
    }

    async fn handle_top_level_frontend_event(
        &mut self,
        event: TopLevelFrontendEvent,
    ) -> Result<()> {
        debug!("Handling top-level frontend event: {:?}", event);
        match event {
            TopLevelFrontendEvent::OpenStream {
                stream_id,
                on_pane_id,
            } => {
                trace!("Opening stream for pane ID: {:?}", on_pane_id);
                let tcp_stream = self
                    .pending_streams
                    .remove(&stream_id)
                    .expect("No pending stream found for pane ID");

                // TODO
                //let (stream_ctrl_tx, stream_ctrl_rx) = unbounded_channel::<StreamCtrl>();
                let (frontend_comm_side, backend_comm_side) =
                    FrontendBackendComm::for_stream(stream_id, on_pane_id);

                Stream::new(
                    stream_id,
                    tcp_stream,
                    on_pane_id,
                    backend_comm_side,
                    self.tokio_egui_bridge.clone(),
                )
                .run()
                .await;

                self.to_frontend
                    .send(TopLevelBackendEvent::StreamStarted(frontend_comm_side))?;
            }
            TopLevelFrontendEvent::CloseStream {
                stream_id,
                on_pane_id,
            } => {
                trace!("Closing stream for pane ID: {:?}", on_pane_id);
                let stream = self
                    .streams
                    .get_mut(&stream_id)
                    .expect("No stream found for pane ID");
                info!("Stopping stream with ID {}", stream_id);
                stream.terminate();
                self.streams
                    .remove(&stream_id)
                    .expect("Stream should be in the map");
            }
        }
        Ok(())
    }
}

pub struct LogAppendBuf<T> {
    phantom: std::marker::PhantomData<T>,
}

#[derive(Clone)]
pub struct LogAppendBufWriter<T> {
    inner: Arc<Mutex<VecDeque<T>>>,
}

// low contention, the writer (tcp) should almost always have access to the buffer, the reader is
// clocked and uses swap
#[derive(Clone)]
pub struct LogAppendBufReader<T> {
    inner: Arc<Mutex<VecDeque<T>>>,
}

impl<T> LogAppendBuf<T> {
    pub fn split() -> (LogAppendBufWriter<T>, LogAppendBufReader<T>) {
        let inner = Arc::new(Mutex::new(VecDeque::new()));
        (
            LogAppendBufWriter {
                inner: inner.clone(),
            },
            LogAppendBufReader { inner },
        )
    }
}

impl<T> LogAppendBufWriter<T> {
    pub fn push(&self, item: T) {
        let mut guard = self.inner.lock();
        guard.push_back(item);
    }

    #[allow(unused)]
    pub fn push_batch(&self, items: impl IntoIterator<Item = T>) {
        let mut guard = self.inner.lock();
        guard.extend(items);
    }
}

impl<T> LogAppendBufReader<T> {
    pub fn swap(&self) -> VecDeque<T> {
        let mut guard = self.inner.lock();
        let mut new_buf = VecDeque::new();
        mem::swap(&mut *guard, &mut new_buf);
        new_buf
    }
}

// TODO: rethink this:
//
// tcp task -> async driven,  copies the logs into a shared buffer
// data task -> periodically (timer)  copies the lgos from the tcp task into permanent storage,
// and computes a set of logs with the filters applied. When the filters change, it recomputes the logs
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

    ctrl_rx: UnboundedReceiver<DataTaskCtrl>,

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
        //data_task_ctrl: UnboundedReceiver<DataTaskCtrl>,
        incoming_logs_buffer: LogAppendBufReader<Arc<DashboardEvent>>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            all_logs: VecDeque::new(),
            filtered_logs: VecDeque::new(),
            incoming_logs_buffer,
            ctrl_rx: todo!(),
            egui_ctx,
            cancel,
            comms,
        }
    }

    /// Starts running once a `DataTaskCtrl::Start` message is received. (by the TCP task)
    //pub fn spawn_on(mut self, task_set: &mut JoinSet<std::result::Result<(), StreamError>>) {
    pub fn spawn(mut self) -> JoinHandle<std::result::Result<(), StreamError>> {
        const TICK_INTERVAL_MS: u64 = 100;
        let mut timer = tokio::time::interval(Duration::from_millis(u64::MAX));
        let mut timer_enabled = true; // Start with timer enabled

        tokio::task::spawn(async move {
            //task_set.spawn(async move {
            debug!("Starting DataTask");
            loop {
                tokio::select! {
                    r = self.ctrl_rx.recv() => {
                        match r {
                            Some(DataTaskCtrl::StopTimer) => {
                                info!("Received DataTaskCtrl::Stop, disabling timer");
                                timer_enabled = false;
                                timer = tokio::time::interval(Duration::from_millis(u64::MAX)); // this is shitty but better than Pin
                            }
                            Some(DataTaskCtrl::StartTimer) => {
                                info!("Received DataTaskCtrl::Start, enabling timer");
                                timer_enabled = true;
                                // Reset the timer to start fresh
                                timer = tokio::time::interval(Duration::from_millis(TICK_INTERVAL_MS));
                            }
                            None => {
                                info!("DataTaskCtrl channel closed, exiting loop");
                                return Ok(());
                            }
                        }
                    },
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

type LogCollection = VecDeque<Arc<DashboardEvent>>;

#[derive(Debug, Clone, Default)]
pub struct DataToDisplay {
    // Vector of references to the logs that match the current filter
    // The logs are not actually stored here, so clone is cheap
    pub filtered_logs: LogCollection,
    pub log_counts: LogCounts,

    pub oculus_memory_usage_mb: f64,
}

#[derive(Debug, Clone, Default, Copy)]
pub struct LogCounts {
    pub total: usize,
    pub trace: usize,
    pub debug: usize,
    pub info: usize,
    pub warn: usize,
    pub error: usize,
}

impl LogCounts {
    fn from_logs(logs: &LogCollection) -> Self {
        let mut counts = LogCounts::default();
        for log in logs {
            match log.level {
                argus::tracing::oculus::Level::TRACE => counts.trace += 1,
                argus::tracing::oculus::Level::DEBUG => counts.debug += 1,
                argus::tracing::oculus::Level::INFO => counts.info += 1,
                argus::tracing::oculus::Level::WARN => counts.warn += 1,
                argus::tracing::oculus::Level::ERROR => counts.error += 1,
            }
            counts.total += 1;
        }
        counts
    }

    fn reset(&mut self) {
        *self = LogCounts::default();
    }
}

struct MemoryTracker {
    sys: System,
    pid: sysinfo::Pid,
}

impl Default for MemoryTracker {
    fn default() -> Self {
        let sys = System::new();
        let pid = sysinfo::get_current_pid().expect("Bug: Failed to get current PID");
        Self { sys, pid }
    }
}

impl MemoryTracker {
    fn refresh_processes(&mut self) {
        let _r = self
            .sys
            .refresh_processes(sysinfo::ProcessesToUpdate::Some(&[self.pid]), true);
    }

    fn calc_memory_usage_mb(&mut self) -> f64 {
        self.refresh_processes();
        if let Some(proc) = self.sys.process(self.pid) {
            // memory is in Bytes → convert to MB with decimals
            proc.memory() as f64 / (1024.0 * 1024.0)
        } else {
            0.0
        }
    }
}

// Should be fast
fn contains_case_insensitive(haystack: &str, needle: &str) -> bool {
    let needle_len = needle.len();
    if needle_len == 0 {
        return true;
    }

    haystack
        .as_bytes()
        .windows(needle_len)
        .any(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}
