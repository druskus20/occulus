use std::sync::Arc;

use argus::tracing::oculus::DashboardEvent;
use egui_tiles::TileId;
use futures::FutureExt;
use tokio::{sync::mpsc::unbounded_channel, task::JoinSet};
use tokio_util::sync::CancellationToken;

use crate::{
    async_rt::TokioEguiBridge,
    backend::{DataTask, DataTaskCtrl, LogAppendBuf, TcpTask},
    data::BackendCommForStream,
    prelude::*,
};

use super::LogAppendBufReader;

#[derive(Debug)]
pub struct StreamError {
    stream_id: usize,
    kind: StreamErrorKind,
}

#[derive(Debug)]
pub enum StreamErrorKind {
    TcpTaskFailed,
    DataTaskFailed,
}

impl std::error::Error for StreamError {}
impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StreamError (ID: {}): {:?}", self.stream_id, self.kind)
    }
}

/// A single stream of logs, which consists in two tasks, a data task and a tcp task.
pub struct Stream {
    stream_id: usize,
    pane_id: TileId,

    done_tcp: bool,

    cancel_data: CancellationToken,
    cancel_tcp: CancellationToken,

    //tcp_task: Option<tokio::task::JoinHandle<std::result::Result<(), StreamError>>>,
    //data_task: tokio::task::JoinHandle<std::result::Result<(), StreamError>>,
    tokio_egui_bridge: TokioEguiBridge,
}

struct StreamHandle {
    stream_id: usize,
    pane_id: TileId,
    cancel_data: CancellationToken,
    cancel_tcp: CancellationToken,
    tokio_egui_bridge: TokioEguiBridge,
    tcp_task: Option<tokio::task::JoinHandle<std::result::Result<(), StreamError>>>,
    data_task: tokio::task::JoinHandle<std::result::Result<(), StreamError>>,
}
impl Stream {
    pub fn new(
        stream_id: usize,
        pane_id: TileId,
        comm_with_frontend: BackendCommForStream,
        stream_task_set: &mut JoinSet<std::result::Result<(), StreamError>>,
        incoming_logs_rx: LogAppendBufReader<Arc<DashboardEvent>>,
        data_task_ctrl: tokio::sync::mpsc::UnboundedReceiver<DataTaskCtrl>,
        tokio_egui_bridge: TokioEguiBridge,
    ) -> Self {
        Self {
            stream_id,
            pane_id,
            done_tcp: false,
            cancel_data: CancellationToken::new(),
            cancel_tcp: CancellationToken::new(),
            tokio_egui_bridge,
        }
    }

    pub async fn run(
        &mut self,
        stream_id: usize,
        pane_id: TileId,
        comm_with_frontend: BackendCommForStream,
        stream_task_set: &mut JoinSet<std::result::Result<(), StreamError>>,
        incoming_logs_rx: LogAppendBufReader<Arc<DashboardEvent>>,
        data_task_ctrl: tokio::sync::mpsc::UnboundedReceiver<DataTaskCtrl>,
        tokio_egui_bridge: TokioEguiBridge,
    ) -> std::result::Result<(), StreamError> {
        let egui_ctx = tokio_egui_bridge.wait_egui_ctx().await;

        debug!("Starting new stream with ID {}", stream_id);

        // Communication between tasks
        let (incoming_logs_tx, incoming_logs_rx) = LogAppendBuf::split(); // logs
        let (to_data_ctrl, from_tcp_ctrl) = unbounded_channel::<DataTaskCtrl>(); // ctrl

        // TCP TASK
        let cancel_tcp = CancellationToken::new();
        let tcp_task = TcpTask::new(
            stream_id,
            cancel_tcp.clone(),
            to_data_ctrl,
            incoming_logs_tx,
        )
        .spawn()
        .fuse();

        // DATA TASK
        let cancel_data = CancellationToken::new();
        let data_task = DataTask::new(
            comm_with_frontend,
            egui_ctx,
            data_task_ctrl,
            incoming_logs_rx,
            cancel_data.clone(),
        )
        .spawn()
        .fuse();

        tokio::pin!(tcp_task);
        tokio::pin!(data_task);
        //futures::pin_mut!(tcp_task);
        //futures::pin_mut!(data_task);

        let mut tcp_done = false;
        let mut data_done = false;
        loop {
            tokio::select! {
                r = &mut data_task, if !data_done => {
                    data_done = true;
                    match r {
                        Ok(Ok(())) => debug!("Data task completed successfully"),
                        Ok(Err(e)) => error!("Data task encountered an error"),
                        Err(e) => error!("Data task panicked: {:?}", e),
                    }
                }

                r = &mut tcp_task, if !tcp_done => {
                    self.done_tcp = true;
                    tcp_done = true;
                    match r {
                        Ok(Ok(())) => debug!("TCP task completed successfully"),
                        Ok(Err(e)) => error!("TCP task encountered an error"),
                        Err(e) => error!("TCP task panicked: {:?}", e),
                    }
                }
                else => {
                    // Only return from the steram when both tasks are done
                    break;

                }
            }
        }
        Ok(())
    }

    #[tracing::instrument(skip_all)]
    pub fn terminate(&self) {
        trace!("Terminating stream {}", self.stream_id);
        self.cancel_tcp.cancel();
        self.cancel_data.cancel();
    }
}
