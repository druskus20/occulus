use std::sync::Arc;

use argus::tracing::oculus::DashboardEvent;
use egui_tiles::TileId;
use tokio::{sync::mpsc::unbounded_channel, task::JoinSet};
use tokio_util::sync::CancellationToken;

use crate::{
    async_rt::TokioEguiBridge,
    backend::{DataTask, DataTaskCtrl, LogAppendBuf, TcpTask},
    data::BackendCommForStream,
    prelude::*,
};

use super::{LogAppendBufReader, StreamError};

/// A single stream of logs, which consists in two tasks, a data task and a tcp task.
pub struct Stream {
    stream_id: usize,
    pane_id: TileId,

    cancel_data: CancellationToken,
    cancel_tcp: CancellationToken,

    tokio_egui_bridge: TokioEguiBridge,
}

impl Stream {
    async fn start_new(
        stream_id: usize,
        pane_id: TileId,
        comm_with_frontend: BackendCommForStream,
        stream_task_set: &mut JoinSet<std::result::Result<(), StreamError>>,
        cancel_tcp: CancellationToken,
        incoming_logs_rx: LogAppendBufReader<Arc<DashboardEvent>>,
        from_tcp_ctrl: tokio::sync::mpsc::UnboundedReceiver<DataTaskCtrl>,
        tokio_egui_bridge: TokioEguiBridge,
    ) -> Result<Self> {
        let egui_ctx = tokio_egui_bridge.wait_egui_ctx().await;

        debug!("Starting new stream with ID {}", stream_id);

        // Communication between tasks
        //let (incoming_logs_tx, incoming_logs_rx) = LogAppendBuf::split(); // logs
        //let (to_data_ctrl, from_tcp_ctrl) = unbounded_channel::<DataTaskCtrl>(); // ctrl

        // TCP TASK
        //let cancel_tcp = CancellationToken::new();
        //TcpTask::new(
        //    stream_id,
        //    cancel_tcp.clone(),
        //    to_data_ctrl,
        //    incoming_logs_tx,
        //)
        //.spawn_on(stream_task_set);

        // DATA TASK
        let cancel_data = CancellationToken::new();
        DataTask::new(
            comm_with_frontend,
            egui_ctx,
            from_tcp_ctrl,
            incoming_logs_rx,
            cancel_data.clone(),
        )
        .spawn_on(stream_task_set);

        Ok(Self {
            stream_id,
            pane_id,
            cancel_data,
            cancel_tcp,
            tokio_egui_bridge,
        })
    }

    fn is_receiving_tcp(&self) -> bool {
        self.cancel_tcp.is_cancelled()
    }

    fn stop_receiving_tcp(&self) {
        if !self.is_receiving_tcp() {
            debug!("Stopping TCP task for stream {}", self.stream_id);
            self.cancel_tcp.cancel();
        } else {
            warn!("TCP task for stream {} is already inactive", self.stream_id);
        }
    }

    #[tracing::instrument(skip_all)]
    fn terminate(&self) {
        trace!("Terminating stream {}", self.stream_id);
        self.stop_receiving_tcp();
        self.cancel_data.cancel();
    }
}
