#[allow(unused_imports)]
pub(crate) mod prelude {
    pub(crate) type Result<T> = color_eyre::Result<T>;
    pub(crate) use color_eyre::eyre::Context;
    pub(crate) use color_eyre::eyre::eyre;
    pub(crate) use tracing::{debug, error, info, trace, warn};
}

use self::prelude::*;
use async_rt::TokioEguiBridge;
use backend::{Backend, TopLevelBackendEvent};
use frontend::TopLevelFrontendEvent;
use tokio::sync::mpsc::unbounded_channel;

mod async_rt;
mod backend;
mod cli;
mod data;
pub mod frontend;
mod oneshot_notify;

fn main() -> Result<()> {
    // Init stuff
    color_eyre::install()?;
    let args = cli::ParsedArgs::parse_raw(); // cli args
    dbg!(&args);
    quill::init(args.tracing_options.color); // color logging utilities
    let _guard = argus::tracing::setup_tracing(&args.tracing_options); // tracing

    // This piece links together Egui and Tokio. It does several things:
    // 1) Startup syncronization - once Egui is ready it gives a reference to the Egui context to Tokio.
    // 3) Cancellation both ways - the GUI can cancel the tokio tasks, and the ctrl-c handler can cancel the GUI.
    let tokio_egui_bridge = TokioEguiBridge::new();

    // Cancellation
    spells::ctrl_c::install_ctrlc_handler_f({
        let tokio_egui_bridge = tokio_egui_bridge.clone();
        move || {
            info!("Ctrl-C pressed, shutting down...");
            tokio_egui_bridge.cancel();
        }
    });

    let (to_backend, from_frontend) = unbounded_channel::<TopLevelFrontendEvent>();
    let (to_frontend, from_backend) = unbounded_channel::<TopLevelBackendEvent>();

    // TOKIO - Background threads
    let tokio_thread_handle = {
        let tokio_egui_bridge = tokio_egui_bridge.clone(); // take ownership 
        async_rt::start(tokio_egui_bridge.clone(), async move {
            let egui_ctx = tokio_egui_bridge.wait_egui_ctx().await;
            let mut backend =
                Backend::init(egui_ctx, from_frontend, to_frontend, tokio_egui_bridge).await;
            backend.run().await
        })
    };

    // EGUI - Main thread
    match args.command {
        cli::Command::Launch => {
            frontend::run_egui(to_backend, from_backend, tokio_egui_bridge.clone())?
        }
    };

    // Join the tokio threads
    tokio_thread_handle
        .join()
        .map_err(|e| eyre!("Tokio thread panicked: {:?}", e))?;

    info!("Tokio thread finished successfully.");

    Ok(())
}
