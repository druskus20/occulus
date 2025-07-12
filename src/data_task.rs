use crate::egui_app::{self, LogDisplaySettings, UiEvent, create_layout_job};
use crate::prelude::*;
use argus::tracing::oculus::DashboardEvent;
use egui::mutex::Mutex;
use std::collections::VecDeque;
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use std::mem;

pub struct LogAppendBuf<T> {
    inner: Arc<Mutex<VecDeque<T>>>,
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
    pub fn new() -> (LogAppendBufWriter<T>, LogAppendBufReader<T>) {
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

    // TODO!!!!!!!! - we need to distinghish between "all the logs" and the "logs to display"
    // (which match the filter). Both buffers need to be updated whenever the tick happens.
    // The filtered logs are updated when the filters change.
    all_logs: VecDeque<Arc<DashboardEvent>>,
    // Logs to display, a reference to the logs that match the current filter
    filtered_logs: VecDeque<Arc<DashboardEvent>>,
    display_data_tx: triple_buffer::Input<DisplayData>,

    data_task_ctrl: UnboundedReceiver<DataTaskCtrl>,

    from_ui: UnboundedReceiver<UiEvent>,
    log_display_settings: RwLock<LogDisplaySettings>,
    egui_ctx: egui::Context,
    cancel: CancellationToken,
}

pub struct TcpTask {
    cancel: CancellationToken,
    data_task_ctrl_tx: UnboundedSender<DataTaskCtrl>,
    incoming_logs_writer: LogAppendBufWriter<Arc<DashboardEvent>>,
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

#[derive(Debug)]
pub struct DataUiBridge {
    /// Shared buffer to publish logs to the EGUI context
    display_data_tx: triple_buffer::Input<DisplayData>,
    /// Initial log display settings (controlled by the UI)
    log_display_settings: LogDisplaySettings,
    /// Receiver for events from the UI
    from_ui: UnboundedReceiver<egui_app::UiEvent>,

    /// Tokio <-> EGUI bridge to handle EGUI context initialization and cancellation
    egui_ctx: egui::Context,
    /// Cancellation token to signal task cancellation
    cancel: CancellationToken,
}

impl DataUiBridge {
    pub fn new(
        egui_ctx: egui::Context,
        display_data_tx: triple_buffer::Input<DisplayData>,
        log_display_settings: LogDisplaySettings,
        from_ui: UnboundedReceiver<egui_app::UiEvent>,
    ) -> Self {
        Self {
            display_data_tx,
            log_display_settings,
            from_ui,
            egui_ctx,
            cancel: CancellationToken::new(),
        }
    }
}

impl DataPrecomputeTask {
    pub fn new(
        cancel: CancellationToken,
        data_task_ctrl: UnboundedReceiver<DataTaskCtrl>,
        incoming_logs_buffer: LogAppendBufReader<Arc<DashboardEvent>>,
        data_ui_bridge: DataUiBridge,
    ) -> Self {
        Self {
            all_logs: VecDeque::new(),
            filtered_logs: VecDeque::new(),
            incoming_logs_buffer,
            data_task_ctrl,
            from_ui: data_ui_bridge.from_ui,
            log_display_settings: RwLock::new(data_ui_bridge.log_display_settings),
            egui_ctx: data_ui_bridge.egui_ctx.clone(),
            display_data_tx: data_ui_bridge.display_data_tx,
            cancel: data_ui_bridge.cancel.clone(),
        }
    }

    async fn data_ui_bridge(self) -> DataUiBridge {
        DataUiBridge {
            display_data_tx: self.display_data_tx,
            log_display_settings: *self.log_display_settings.read().await,
            from_ui: self.from_ui,
            egui_ctx: self.egui_ctx.clone(),
            cancel: self.cancel.clone(),
        }
    }

    /// Starts running once a `DataTaskCtrl::Start` message is received. (by the TCP task)
    pub fn spawn(mut self) -> JoinHandle<std::result::Result<DataUiBridge, DataTaskError>> {
        const TICK_INTERVAL_MS: u64 = 100;
        let mut timer = tokio::time::interval(Duration::from_millis(u64::MAX));
        let mut timer_enabled = true; // Start with timer enabled

        tokio::task::spawn(async move {
            loop {
                tokio::select! {
                    r = self.data_task_ctrl.recv() => {
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
                                return Ok(self.data_ui_bridge().await);
                            }
                        }
                    },
                    r = self.from_ui.recv() => {
                        match r {
                            Some(event) => {
                                trace!("Received UI event: {:?}", event);
                                self.handle_ui_event(event).await;
                            }
                            None => {
                                info!("UI event channel closed, exiting loop");
                                return Ok(self.data_ui_bridge().await);
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
                                    _owned_data_ui_bridge: self.data_ui_bridge().await,
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
        let log_display_settings = *self.log_display_settings.read().await;

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
            .filter(|event| event.level >= log_display_settings.level_filter.into())
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
        *self.display_data_tx.input_buffer_mut() = DisplayData {
            filtered_logs: self.filtered_logs.clone(),
            log_counts: LogCounts::from_logs(&self.all_logs),
        };
        self.display_data_tx.publish();
        self.egui_ctx.request_repaint();

        Ok(())
    }

    async fn handle_ui_event(&mut self, event: UiEvent) {
        match event {
            UiEvent::LogDisplaySettingsChanged(new_settings) => {
                *self.log_display_settings.write().await = new_settings;
                self.filter_and_refresh_egui_buf().await;
            }
        }
        info!("UI message handler disconnected, exiting loop");
    }

    async fn filter_and_refresh_egui_buf(&mut self) {
        info!("Refreshing EGUI buffer");
        let settings = *self.log_display_settings.read().await;
        let all_logs = &self.all_logs;

        let log_count = LogCounts::from_logs(all_logs);
        let filtered_logs = all_logs
            .iter()
            .filter(|&event| event.level >= settings.level_filter.into())
            .cloned()
            .collect::<VecDeque<_>>();

        *self.display_data_tx.input_buffer_mut() = DisplayData {
            filtered_logs,
            log_counts: log_count,
        };
        // Refresh the triple buffer to reflect the new settings
        self.display_data_tx.publish();
        self.egui_ctx.request_repaint();
    }
}

#[derive(Debug)]
pub struct DataTaskError {
    msg: String,
    /// Passes the ownerhip back in case of an error so that it can be reused for another
    /// connection later on
    _owned_data_ui_bridge: DataUiBridge,
}

impl Error for DataTaskError {}
impl std::fmt::Display for DataTaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DataTaskError: {}", self.msg)
    }
}
#[derive(Debug, Clone, Default)]
pub struct OculusInternalMetrics {
    incoming_data_count: u32,
}

#[derive(Debug, Clone, Default)]
pub struct DisplayData {
    // Vector of references to the logs that match the current filter
    // The logs are not actually stored here, so clone is cheap
    pub filtered_logs: VecDeque<Arc<DashboardEvent>>,
    pub log_counts: LogCounts,
}

impl LogCounts {
    fn from_logs(logs: &VecDeque<Arc<DashboardEvent>>) -> Self {
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
}

#[derive(Debug, Clone, Default)]
pub struct LogCounts {
    pub total: usize,
    pub trace: usize,
    pub debug: usize,
    pub info: usize,
    pub warn: usize,
    pub error: usize,
}
