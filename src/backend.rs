use crate::async_rt::TokioEguiBridge;
use crate::frontend::UiEvent;
use crate::{BackendSide, prelude::*};
use argus::tracing::oculus::DashboardEvent;
use egui::mutex::Mutex;
use std::collections::VecDeque;
use std::error::Error;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use std::{env, mem};
use sysinfo::System;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

/// Runs the backend-side tasks for handling TCP connections and data processing
pub async fn run_backend(backend: BackendSide, tokio_egui_bridge: TokioEguiBridge) {
    let cancel = tokio_egui_bridge.cancel_token();

    // Communication between tasks
    let (to_data_ctrl, from_tcp_ctrl) = unbounded_channel::<DataTaskCtrl>(); // ctrl
    let (incoming_logs_tx, incoming_logs_rx) = LogAppendBuf::split(); // logs

    // TCP TASK
    let _tcp_task = TcpTask::new(cancel.clone(), to_data_ctrl, incoming_logs_tx).spawn();

    // DATA TASK
    // Wait for egui to be initialized
    let egui_ctx = tokio_egui_bridge.wait_egui_ctx().await;
    let _data_precompute_task_handle = DataPrecomputeTask::new(
        backend,
        egui_ctx,
        from_tcp_ctrl,
        incoming_logs_rx,
        cancel.clone(),
    )
    .spawn();

    // Cancel
    let cancel = tokio_egui_bridge.cancelled_fut();
    tokio::select! {
        _ = cancel => {
            warn!("Tokio task cancelled, shutting down...");
        },
    };
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
pub struct DataPrecomputeTask {
    incoming_logs_buffer: LogAppendBufReader<Arc<DashboardEvent>>,

    /// All logs, kept in memory for the lifetime of the task
    all_logs: LogCollection,
    /// Filtered logs, matching the current filter. These are the logs that will be displayed in
    /// the UI.
    ///
    /// TODO: this might be redundant with the display_data_tx (in backend)
    filtered_logs: LogCollection,

    ctrl_rx: UnboundedReceiver<DataTaskCtrl>,

    backend_side: BackendSide,
    egui_ctx: egui::Context,

    memory_tracker: MemoryTracker,

    #[allow(unused)]
    cancel: CancellationToken,
}

pub struct TcpTask {
    data_task_ctrl_tx: UnboundedSender<DataTaskCtrl>,
    incoming_logs_writer: LogAppendBufWriter<Arc<DashboardEvent>>,
    #[allow(unused)]
    cancel: CancellationToken,
}

pub enum DataTaskCtrl {
    StartTimer,
    StopTimer,
}

impl TcpTask {
    pub fn new(
        cancel: CancellationToken,
        data_task_ctrl_tx: UnboundedSender<DataTaskCtrl>,
        incoming_logs_writer: LogAppendBufWriter<Arc<DashboardEvent>>,
    ) -> Self {
        Self {
            cancel,
            data_task_ctrl_tx,
            incoming_logs_writer,
        }
    }

    pub fn spawn(self) -> JoinHandle<Result<()>> {
        tokio::task::spawn(async move {
            self.tcp_loop().await.wrap_err("TCP task failed")?;
            Ok(())
        })
    }

    async fn tcp_loop(&self) -> Result<()> {
        // accept tpc connections in loop - only one at a time
        loop {
            let listener = TcpListener::bind("127.0.0.1:8080").await?;
            info!("Listening for incoming TCP connections on 127.0.0.1:8080");

            let (stream, addr) = listener.accept().await?;
            info!("Accepted connection from {}", addr);
            self.data_task_ctrl_tx.send(DataTaskCtrl::StartTimer)?;

            // Forward messages
            self.handle_log_stream(stream)
                .await
                .wrap_err(format!("Connection handling failed {addr}"))?;

            info!("Connection from {} closed, stopping data task", addr);
            self.data_task_ctrl_tx.send(DataTaskCtrl::StopTimer)?;
        }
    }

    async fn handle_log_stream(&self, stream: TcpStream) -> Result<()> {
        let reader = BufReader::new(stream);
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            let event = serde_json::from_str::<DashboardEvent>(&line)?;
            self.incoming_logs_writer.push(Arc::new(event));
        }

        Ok(())
    }
}

impl DataPrecomputeTask {
    pub fn new(
        backend: BackendSide,
        egui_ctx: egui::Context,
        data_task_ctrl: UnboundedReceiver<DataTaskCtrl>,
        incoming_logs_buffer: LogAppendBufReader<Arc<DashboardEvent>>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            all_logs: VecDeque::new(),
            filtered_logs: VecDeque::new(),
            incoming_logs_buffer,
            ctrl_rx: data_task_ctrl,
            egui_ctx,
            backend_side: backend,
            cancel,
            memory_tracker: MemoryTracker::default(),
        }
    }

    /// Starts running once a `DataTaskCtrl::Start` message is received. (by the TCP task)
    /// Returns the BackendSide, which can be reused for another connection later on.
    pub fn spawn(mut self) -> JoinHandle<std::result::Result<BackendSide, DataTaskError>> {
        const TICK_INTERVAL_MS: u64 = 100;
        let mut timer = tokio::time::interval(Duration::from_millis(u64::MAX));
        let mut timer_enabled = true; // Start with timer enabled

        tokio::task::spawn(async move {
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
                                return Ok(self.backend_side);
                            }
                        }
                    },
                    r = self.backend_side.from_frontend.recv() => {
                        match r {
                            Some(event) => {
                                trace!("Received UI event: {:?}", event);
                                self.handle_ui_event(event).await;
                            }
                            None => {
                                info!("UI event channel closed, exiting loop");
                                return Ok(self.backend_side);
                            }
                        }
                    },
                    _ = timer.tick(), if timer_enabled => {
                        match self.publish_new_logs().await {
                            Ok(_) => trace!("Published new logs to EGUI context"),
                            Err(e) => {
                                error!("Failed to publish new logs: {:?}", e);
                                return Err(DataTaskError {
                                    msg: format!("Failed to publish new logs: {e:?}"),
                                    backend: self.backend_side,
                                });
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
        let log_display_settings = self.backend_side.settings.clone();

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
                    && if !self.backend_side.settings.search_string.is_empty() {
                        contains_case_insensitive(
                            event.message.as_str(),
                            self.backend_side.settings.search_string.as_str(),
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
        let oculus_memory_usage_mb = self.memory_tracker.calc_memory_usage_mb();
        *self.backend_side.data_buffer_tx.input_buffer_mut() = DataToDisplay {
            filtered_logs: self.filtered_logs.clone(),
            log_counts: LogCounts::from_logs(&self.all_logs),
            oculus_memory_usage_mb,
        };
        self.backend_side.data_buffer_tx.publish();
        self.egui_ctx.request_repaint();

        Ok(())
    }

    async fn handle_ui_event(&mut self, event: UiEvent) {
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
            UiEvent::LogDisplaySettingsChanged(new_settings) => {
                info!("Received new log display settings: {:?}", new_settings);
                self.backend_side.settings = new_settings;
                self.filter_and_refresh_egui_buf().await;
            }
            UiEvent::OpenInEditor { path, line } => {
                // verify that the file exists
                let path = std::path::PathBuf::from(path);
                if !path.exists() {
                    error!("File does not exist: {}", path.display());
                    return;
                }
                open_in_nvim(&path.to_string_lossy(), line);
            }
            UiEvent::Clear => {
                info!("Clearing all logs");
                self.all_logs.clear();
                self.filtered_logs.clear();
                // Reset the data buffer
                self.backend_side
                    .data_buffer_tx
                    .input_buffer_mut()
                    .filtered_logs
                    .clear();
                self.backend_side
                    .data_buffer_tx
                    .input_buffer_mut()
                    .log_counts
                    .reset();

                self.backend_side.data_buffer_tx.publish();
                self.egui_ctx.request_repaint();
            }
        }
    }

    async fn filter_and_refresh_egui_buf(&mut self) {
        trace!("Refreshing EGUI buffer");
        let all_logs = &self.all_logs;

        let log_count = LogCounts::from_logs(all_logs);
        let filtered_logs = all_logs
            .iter()
            .filter(|event| {
                (event.level >= self.backend_side.settings.level_filter.into())
                    && if !self.backend_side.settings.search_string.is_empty() {
                        contains_case_insensitive(
                            event.message.as_str(),
                            self.backend_side.settings.search_string.as_str(),
                        )
                    } else {
                        true
                    }
            })
            .cloned()
            .collect::<VecDeque<_>>();

        let oculus_memory_usage_mb = self.memory_tracker.calc_memory_usage_mb();
        *self.backend_side.data_buffer_tx.input_buffer_mut() = DataToDisplay {
            filtered_logs,
            log_counts: log_count,
            oculus_memory_usage_mb,
        };
        // Refresh the triple buffer to reflect the new settings
        self.backend_side.data_buffer_tx.publish();
        self.egui_ctx.request_repaint();
    }
}

#[derive(Debug)]
pub struct DataTaskError {
    msg: String,
    /// Passes the ownerhip back in case of an error so that it can be reused for another
    /// connection later on
    #[allow(unused)]
    backend: BackendSide,
}

impl Error for DataTaskError {}
impl std::fmt::Display for DataTaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DataTaskError: {}", self.msg)
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
