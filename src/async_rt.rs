use std::{
    num::NonZeroUsize,
    sync::{Arc, OnceLock},
    thread::JoinHandle,
};

use crate::{oneshot_notify::OneshotNotify, prelude::*};
use tokio::runtime::{Builder, Handle, Runtime};
use tokio_util::sync::CancellationToken;

pub fn start(
    tokio_egui_bridge: TokioEguiBridge,
    fut: impl futures::Future<Output = ()> + Send + 'static,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let num_cpus = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
        info!(
            "Using {} - {} worker threads for the tokio runtime",
            { num_cpus },
            { 1 }
        );
        let rt = Builder::new_multi_thread()
            .enable_io()
            .enable_time()
            .worker_threads(num_cpus - 1) // reserve one for gui (main) thread
            .build()
            .expect("Failed to create tokio runtime");

        // Pass the runtime to the bridge so that it can be used later
        // to spawn tasks on the runtime from the GUI thread.
        tokio_egui_bridge.register_tokio_runtime(rt.handle().clone());

        rt.block_on(fut);
    })
}

#[derive(Debug, Clone)]
pub struct TokioEguiBridge {
    /// This field starts as `None` and is set when the Egui context is registered.
    /// A notification is sent when the context is available.
    egui_ctx: Arc<std::sync::OnceLock<egui::Context>>,
    egui_ctx_available: Arc<OneshotNotify>,

    /// This field starts as `None` and is set when the Tokio runtime is initialized.
    /// A notification is sent when the runtime is available.
    tokio_rt: Arc<std::sync::OnceLock<tokio::runtime::Handle>>,
    tokio_rt_available: Arc<OneshotNotify>,

    cancel: CancellationToken,
}

impl Default for TokioEguiBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl TokioEguiBridge {
    pub fn new() -> Self {
        Self {
            egui_ctx: Arc::new(OnceLock::new()),
            egui_ctx_available: Arc::new(OneshotNotify::new()),
            tokio_rt: Arc::new(OnceLock::new()),
            tokio_rt_available: Arc::new(OneshotNotify::new()),
            cancel: CancellationToken::new(),
        }
    }

    pub fn register_egui_context(&self, ctx: egui::Context) {
        info!("Registering Egui context");
        self.egui_ctx.set(ctx).expect("Egui context already set");
        info!("Egui context registered successfully");
        self.egui_ctx_available.notify();
    }

    pub async fn wait_egui_ctx(&self) -> egui::Context {
        self.egui_ctx_available.wait().await;
        self.egui_ctx.get().expect("Egui context not set").clone()
    }

    pub fn wait_egui_ctx_blocking(&self) -> egui::Context {
        self.egui_ctx_available.wait_blocking();
        self.egui_ctx.get().expect("Egui context not set").clone()
    }

    pub fn register_tokio_runtime(&self, rt: Handle) {
        info!("Registering Tokio runtime");
        self.tokio_rt.set(rt).expect("Tokio runtime already set");
        self.tokio_rt_available.notify();
    }

    pub async fn wait_tokio_rt(&self) -> Handle {
        self.tokio_rt_available.wait().await;
        self.tokio_rt.get().expect("Tokio runtime not set").clone()
    }

    pub fn wait_tokio_rt_blocking(&self) -> Handle {
        self.tokio_rt_available.wait_blocking();
        self.tokio_rt.get().expect("Tokio runtime not set").clone()
    }

    // resturns a future that resolves when the tokio task is cancelled
    pub fn cancelled_fut(&self) -> impl futures::Future<Output = ()> {
        self.cancel.cancelled()
    }

    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
        match self.egui_ctx.get() {
            Some(ctx) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            None => {
                warn!("Egui context not set, cannot send close command.");
            }
        }
    }
}
