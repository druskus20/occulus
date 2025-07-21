//! Raw TCP handling - simple enough to hand roll it without axum

use std::sync::Arc;

use crate::prelude::*;
use argus::tracing::oculus::DashboardEvent;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{Receiver, Sender, UnboundedSender};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

use super::{DataTaskCtrl, LogAppendBufWriter, StreamError};

pub struct TcpHandlerTask {
    tcp_stream: TcpStream,
    stream_id: usize,

    data_task_ctrl_tx: UnboundedSender<DataTaskCtrl>,
    incoming_logs_writer: LogAppendBufWriter<Arc<DashboardEvent>>,
    cancel: CancellationToken,
}

impl TcpHandlerTask {
    pub fn new(
        stream_id: usize,
        data_task_ctrl_tx: UnboundedSender<DataTaskCtrl>,
        incoming_logs_writer: LogAppendBufWriter<Arc<DashboardEvent>>,
        tcp_stream: TcpStream,
        cancel: CancellationToken,
    ) -> Self {
        TcpHandlerTask {
            tcp_stream,
            cancel,
            stream_id,
            data_task_ctrl_tx,
            incoming_logs_writer,
        }
    }

    pub async fn spawn_on(&self, join_set: &mut JoinSet<std::result::Result<(), StreamError>>) {
        join_set.spawn(async move {
            loop {
                info!("woo!");
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                todo!("Actually handle the stream here");
            }
        });
    }
}
