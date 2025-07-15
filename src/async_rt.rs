use std::{
    num::NonZeroUsize,
    sync::{Arc, OnceLock},
    thread::JoinHandle,
};

use crate::{oneshot_notify::OneshotNotify, prelude::*};
use tokio::runtime::Builder;
use tokio_util::sync::CancellationToken;

pub fn start(fut: impl futures::Future<Output = ()> + Send + 'static) -> JoinHandle<()> {
    //fn start_async_rt(fut: impl futures::Future<Output = ()> + Send + 'static) -> JoinHandle<()> {
    //fn start_async_rt(backend: BackendSide, tokio_egui_bridge: TokioEguiBridge) -> JoinHandle<()> {
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

        rt.block_on(fut);
    })
}

#[derive(Debug, Clone)]
pub struct TokioEguiBridge {
    egui_ctx: Arc<std::sync::OnceLock<egui::Context>>,
    egui_ctx_available: Arc<OneshotNotify>,
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
