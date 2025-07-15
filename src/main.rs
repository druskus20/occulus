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
    DataPrecomputeTask, DataTaskCtrl, DataUiBridge, DisplayData, LogAppendBuf, LogCounts,
    OculusInternalMetrics, TcpTask,
};
use egui::{ahash::HashMap, text::LayoutJob};
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
    let _guard = argus::tracing::setup_tracing(&Default::default());
    info!("Ocular started with args: {:#?}", args);

    // EGUI
    match args.command {
        cli::Command::Launch => egui_app::run_egui(),
    }?;

    info!("Tokio thread finished successfully.");

    Ok(())
}
