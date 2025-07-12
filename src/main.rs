#[allow(unused_imports)]
pub(crate) mod prelude {
    pub(crate) type Result<T> = color_eyre::Result<T>;
    pub(crate) use color_eyre::eyre::Context;
    pub(crate) use color_eyre::eyre::eyre;
    pub(crate) use tracing::{debug, error, info, trace, warn};
}
use std::{
    collections::VecDeque,
    marker::PhantomData,
    num::NonZeroUsize,
    sync::{Arc, Condvar, Mutex, OnceLock, atomic::AtomicBool},
    thread::JoinHandle,
};

use self::prelude::*;
use argus::tracing::oculus::DashboardEvent;
use data_task::{
    DataPrecomputeTask, DataTaskCtrl, DataUiBridge, DisplayData, LogAppendBuf,
    OculusInternalMetrics, TcpTask,
};
use egui::text::LayoutJob;
use egui_app::LogDisplaySettings;
use tokio::{
    runtime::Builder,
    sync::{Notify, mpsc::UnboundedReceiver},
};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
mod cli;
mod data_task;
pub mod egui_app;

fn main() -> Result<()> {
    color_eyre::install()?;
    let args = cli::ParsedArgs::parse_raw();
    quill::init(args.tracing_options.color);
    let _guard = argus::tracing::setup_tracing(&args.tracing_options);
    info!("Ocular started with args: {:#?}", args);

    let tokio_egui_bridge = TokioEguiBridge::new();
    spells::ctrl_c::install_ctrlc_handler_f({
        let tokio_egui_bridge = tokio_egui_bridge.clone();
        move || {
            info!("Ctrl-C pressed, shutting down...");
            tokio_egui_bridge.cancel();
        }
    });

    let initial_log_display_settings = LogDisplaySettings::default();
    let (display_data_tx, display_data_rx) = triple_buffer::triple_buffer(&DisplayData::default());
    let (metrics_buffer_tx, metrics_buffer_rx) =
        triple_buffer::triple_buffer(&PhantomData::default());
    let (metrics_tx, metrics_rx) = triple_buffer::triple_buffer(&PhantomData::<()>::default());
    let (internal_metrics_tx, internal_metrics_rx) =
        triple_buffer::triple_buffer(&OculusInternalMetrics::default());

    let (to_data, from_ui) = tokio::sync::mpsc::unbounded_channel::<egui_app::UiEvent>();

    // TOKIO
    let tokio_thread_handle = run_tokio_thread(
        display_data_tx,
        internal_metrics_tx,
        metrics_buffer_tx,
        tokio_egui_bridge.clone(),
        initial_log_display_settings,
        from_ui,
    );

    // EGUI
    match args.command {
        cli::Command::Launch => egui_app::run_egui(
            display_data_rx,
            internal_metrics_rx,
            metrics_buffer_rx,
            tokio_egui_bridge,
            initial_log_display_settings,
            to_data,
        ),
    }?;

    tokio_thread_handle
        .join()
        .map_err(|e| eyre!("Tokio thread panicked: {:?}", e))?;

    info!("Tokio thread finished successfully.");

    Ok(())
}

fn run_tokio_thread(
    display_data_tx: triple_buffer::Input<DisplayData>,
    internal_metrics_tx: triple_buffer::Input<OculusInternalMetrics>,
    metrics_buffer_tx: triple_buffer::Input<PhantomData<()>>,
    tokio_egui_bridge: TokioEguiBridge,
    initial_log_display_settings: LogDisplaySettings,
    from_ui: UnboundedReceiver<egui_app::UiEvent>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        // default number of threads in the system -1
        let num_cpus = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
        info!(
            "Using {} - {} worker threads for the tokio runtime",
            { num_cpus },
            { 1 }
        );
        let rt = Builder::new_multi_thread()
            .enable_io()
            .enable_time()
            .worker_threads(num_cpus - 1)
            .build()
            .expect("Failed to create tokio runtime");

        rt.block_on(async {
            // Communication between tasks
            let (data_task_ctrl_tx, data_task_ctrl_rx) =
                tokio::sync::mpsc::unbounded_channel::<DataTaskCtrl>();
            let (incoming_logs_writer, incoming_logs_reader) = LogAppendBuf::new();

            // TCP TASK
            let _tcp_task = TcpTask::new(
                tokio_egui_bridge.cancel_token(),
                data_task_ctrl_tx,
                incoming_logs_writer,
            )
            .spawn();

            // DATA TASK
            let egui_ctx = tokio_egui_bridge.wait_egui_ctx().await;
            let data_ui_bridge = DataUiBridge::new(
                egui_ctx,
                display_data_tx,
                initial_log_display_settings,
                from_ui,
            );
            let _data_precompute_task_handle = DataPrecomputeTask::new(
                tokio_egui_bridge.cancel_token(),
                data_task_ctrl_rx,
                incoming_logs_reader,
                data_ui_bridge,
            )
            .spawn();

            // Cancel
            let cancel = tokio_egui_bridge.cancelled_fut();
            tokio::select! {
                // You can add other async tasks here if needed
                _ = cancel => {
                    warn!("Tokio task cancelled, shutting down...");
                },
            };
        });
    })
}

#[derive(Debug, Clone)]
pub struct TokioEguiBridge {
    egui_ctx: Arc<std::sync::OnceLock<egui::Context>>,
    egui_ctx_available: Arc<OneshotNotify>,
    cancel: CancellationToken,
}

/// Like a notify that keeps returning true once it has been notified
#[derive(Debug)]
struct OneshotNotify {
    flag: AtomicBool,
    notify: Notify,
    condvar: (Mutex<bool>, Condvar),
}

impl OneshotNotify {
    fn new() -> Self {
        Self {
            flag: AtomicBool::new(false),
            notify: Notify::new(),
            condvar: (Mutex::new(false), Condvar::new()),
        }
    }

    fn notify(&self) {
        if !self.flag.swap(true, std::sync::atomic::Ordering::AcqRel) {
            let (lock, cvar) = &self.condvar;
            let mut notified = lock.lock().unwrap();
            *notified = true;
            // sync notify
            cvar.notify_all();
            // async notify
            self.notify.notify_waiters();
        }
    }

    async fn wait(&self) {
        info!("Waiting for egui ctx");
        // if already notified once, return immediately
        if self.flag.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        self.notify.notified().await;
        info!("Egui ctx is available now");
    }

    fn wait_blocking(&self) {
        // if already notified once, return immediately
        if self.flag.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        // wait on the condition variable
        let (lock, cvar) = &self.condvar;
        let mut notified = lock.lock().unwrap();
        while !*notified {
            notified = cvar.wait(notified).unwrap();
        }
    }
}

impl TokioEguiBridge {
    fn new() -> Self {
        Self {
            egui_ctx: Arc::new(OnceLock::new()),
            egui_ctx_available: Arc::new(OneshotNotify::new()),
            cancel: CancellationToken::new(),
        }
    }

    fn register_egui_context(&self, ctx: egui::Context) {
        info!("Registering Egui context");
        self.egui_ctx.set(ctx).expect("Egui context already set");
        info!("Egui context registered successfully");
        self.egui_ctx_available.notify();
    }

    async fn wait_egui_ctx(&self) -> egui::Context {
        self.egui_ctx_available.wait().await;
        self.egui_ctx.get().expect("Egui context not set").clone()
    }

    fn wait_egui_ctx_blocking(&self) -> egui::Context {
        self.egui_ctx_available.wait_blocking();
        self.egui_ctx.get().expect("Egui context not set").clone()
    }

    // resturns a future that resolves when the tokio task is cancelled
    fn cancelled_fut(&self) -> impl futures::Future<Output = ()> {
        self.cancel.cancelled()
    }

    fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    fn cancel(&self) {
        self.cancel.cancel();
        match self.egui_ctx.get() {
            Some(ctx) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            None => {
                warn!("Egui context not set, cannot send close command.");
            }
        }
    }
}
