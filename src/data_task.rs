use crate::egui_app::{self, LogDisplaySettings, UiEvent, create_layout_job};
use crate::{TokioEguiBridge, prelude::*};
use argus::tracing::oculus::DashboardEvent;
use egui::TextFormat;
use egui::mutex::Mutex;
use std::collections::VecDeque;
use std::error::Error;
use std::io;
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
    incoming_logs_buffer: LogAppendBufReader<DashboardEvent>,

    // TODO!!!!!!!! - we need to distinghish between "all the logs" and the "logs to display"
    // (which match the filter). Both buffers need to be updated whenever the tick happens.
    // The filtered logs are updated when the filters change.
    all_logs_buffer: Arc<Mutex<VecDeque<DashboardEvent>>>,
    // Logs to display, a reference to the logs that match the current filter
    filtered_logs: VecDeque<DashboardEvent>,
    egui_log_buffer_rx: triple_buffer::Input<VecDeque<DashboardEvent>>,

    data_task_ctrl: UnboundedReceiver<DataTaskCtrl>,

    from_ui: UnboundedReceiver<UiEvent>,
    log_display_settings: RwLock<LogDisplaySettings>,
    egui_ctx: egui::Context,
    cancel: CancellationToken,
}

pub struct TcpTask {
    cancel: CancellationToken,
}

pub enum DataTaskCtrl {
    Start,
    Stop,
}

impl TcpTask {
    pub fn new(cancel: CancellationToken) -> Self {
        Self { cancel }
    }

    pub fn spawn(self, data_ui_bridge: DataUiBridge) -> JoinHandle<Result<()>> {
        tokio::task::spawn(async move {
            self.tcp_loop(data_ui_bridge)
                .await
                .wrap_err("TCP task failed")?;
            Ok(())
        })
    }

    async fn tcp_loop(&self, data_ui_bridge: DataUiBridge) -> Result<()> {
        let mut data_ui_bridge = data_ui_bridge;
        loop {
            let listener = TcpListener::bind("127.0.0.1:8080").await?;
            info!("Listening for incoming TCP connections on 127.0.0.1:8080");

            let (stream, addr) = listener.accept().await?;
            info!("Accepted connection from {}", addr);

            // Init stuff to bridge tcp and data tasks
            //let egui_ctx = tokio_egui_bridge.wait_egui_ctx().await;
            let (incoming_logs_writer, incoming_logs_reader) = LogAppendBuf::new();

            let (data_task_ctrl_tx, data_task_ctrl_rx) =
                tokio::sync::mpsc::unbounded_channel::<DataTaskCtrl>();

            // Start a data task
            let data_precompute_task_handle =
                DataPrecomputeTask::new(data_task_ctrl_rx, incoming_logs_reader, data_ui_bridge)
                    .spawn();

            // Forward messages
            self.handle_log_stream(stream, incoming_logs_writer, data_task_ctrl_tx)
                .await
                .wrap_err(format!("Connection handling failed {addr}"))?;

            // return back the ownership
            data_ui_bridge = data_precompute_task_handle
                .await
                .wrap_err("Data precompute task failed")??;

            info!("Connection from {} closed", addr);
        }
    }

    async fn handle_log_stream(
        &self,
        stream: TcpStream,
        incoming_logs_writer: LogAppendBufWriter<DashboardEvent>,
        data_task_ctrl_tx: UnboundedSender<DataTaskCtrl>,
    ) -> Result<()> {
        let reader = BufReader::new(stream);
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            let event = serde_json::from_str::<DashboardEvent>(&line)?;
            incoming_logs_writer.push(event);
        }

        Ok(())
    }
}

#[derive(Debug)]
pub struct DataUiBridge {
    /// Shared buffer to publish logs to the EGUI context
    ui_log_buffer_tx: triple_buffer::Input<VecDeque<DashboardEvent>>,
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
        ui_log_buffer_tx: triple_buffer::Input<VecDeque<DashboardEvent>>,
        log_display_settings: LogDisplaySettings,
        from_ui: UnboundedReceiver<egui_app::UiEvent>,
    ) -> Self {
        Self {
            ui_log_buffer_tx,
            log_display_settings,
            from_ui,
            egui_ctx,
            cancel: CancellationToken::new(),
        }
    }
}

impl DataPrecomputeTask {
    pub fn new(
        data_task_ctrl: UnboundedReceiver<DataTaskCtrl>,
        incoming_logs_buffer: LogAppendBufReader<DashboardEvent>,
        data_ui_bridge: DataUiBridge,
    ) -> Self {
        Self {
            all_logs_buffer: Arc::new(Mutex::new(VecDeque::new())),
            filtered_logs: VecDeque::new(),
            incoming_logs_buffer,
            data_task_ctrl,
            from_ui: data_ui_bridge.from_ui,
            log_display_settings: RwLock::new(data_ui_bridge.log_display_settings),
            egui_ctx: data_ui_bridge.egui_ctx.clone(),
            egui_log_buffer_rx: data_ui_bridge.ui_log_buffer_tx,
            cancel: data_ui_bridge.cancel.clone(),
        }
    }

    async fn data_ui_bridge(self) -> DataUiBridge {
        DataUiBridge {
            ui_log_buffer_tx: self.egui_log_buffer_rx,
            log_display_settings: *self.log_display_settings.read().await,
            from_ui: self.from_ui,
            egui_ctx: self.egui_ctx.clone(),
            cancel: self.cancel.clone(),
        }
    }

    /// Starts running once a `DataTaskCtrl::Start` message is received. (by the TCP task)
    pub fn spawn(mut self) -> JoinHandle<std::result::Result<DataUiBridge, DataTaskError>> {
        const TICK_INTERVAL_MS: u64 = 100;
        let mut timer = tokio::time::interval(Duration::from_millis(TICK_INTERVAL_MS));
        tokio::task::spawn(async move {
            loop {
                tokio::select! {
                    r = self.ui_msg_handler_loop() => {
                        match r {
                            Ok(_) =>{
                                info!("UI message handler loop finished successfully");
                                return Ok(self.data_ui_bridge().await);
                            }
                            Err(e) => {
                                info!("UI message handler loop failed: {:?}", e);
                                return Err(DataTaskError {
                                    msg: format!("UI message handler loop failed: {e:?}"),
                                    _owned_data_ui_bridge: self.data_ui_bridge().await,
                                });
                            }
                        }
                    }
                    _ = timer.tick() => {
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
        let filtered_logs = new_logs
            .iter()
            // Filter with the current log display settings
            .filter(|event| event.level >= log_display_settings.level_filter.into())
            .cloned()
            .collect::<Vec<_>>();

        self.all_logs_buffer.lock().extend(new_logs);

        // This avoids swapping the buffers unnecessarily
        if filtered_logs.is_empty() {
            trace!("No new logs to publish (matching the current filter)");
            return Ok(());
        }

        // Update and publish the new logs to the EGUI context
        info!(
            "Requesting EGUI repaint with {} new jobs",
            filtered_logs.len()
        );
        self.filtered_logs.extend(filtered_logs);
        // All the logs - we cannot extend, becasue how triple_buffer is implemented
        *self.egui_log_buffer_rx.input_buffer_mut() = self.filtered_logs.clone();
        self.egui_log_buffer_rx.publish();
        self.egui_ctx.request_repaint();

        Ok(())
    }

    async fn ui_msg_handler_loop(&mut self) -> Result<()> {
        while let Some(event) = self.from_ui.recv().await {
            match event {
                UiEvent::LogDisplaySettingsChanged(new_settings) => {
                    *self.log_display_settings.write().await = new_settings;
                    self.filter_and_refresh_egui_buf().await;
                }
            }
        }
        info!("UI message handler disconnected, exiting loop");
        Ok(())
    }

    async fn filter_and_refresh_egui_buf(&mut self) {
        info!("Refreshing EGUI buffer");
        let settings = *self.log_display_settings.read().await;
        let all_logs = &self.all_logs_buffer.lock();

        let filtered_logs = all_logs
            .iter()
            .filter(|&event| event.level >= settings.level_filter.into())
            .cloned()
            .collect::<VecDeque<_>>();

        *self.egui_log_buffer_rx.input_buffer_mut() = filtered_logs;
        // Refresh the triple buffer to reflect the new settings
        self.egui_log_buffer_rx.publish();
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
