use crate::data::{BackendCommForStream, FrontendBackendComm, FrontendCommForStream, StreamData};
use crate::frontend::{ScopedUIEvent, TopLevelFrontendEvent};
use crate::prelude::*;

use crate::async_rt::TokioEguiBridge;
use argus::tracing::oculus::DashboardEvent;
use egui::ahash::HashMap;
use egui::mutex::Mutex;
use egui_tiles::TileId;
use futures::StreamExt;
use std::collections::VecDeque;
use std::error::Error;
use std::panic::catch_unwind;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use std::{env, mem};
use stream::{Stream, StreamError, StreamErrorKind, StreamHandle};
use sysinfo::System;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

mod stream;

#[derive(Debug)]
pub enum TopLevelBackendEvent {
    NewPendingStream {
        stream_id: usize,
        addr: std::net::SocketAddr,
    },
    StreamStarted(FrontendCommForStream),
}
pub struct BakcendEvent {}

/// Manages multiple streams
pub struct Backend {
    pending_streams: HashMap<usize, TcpStream>,
    streams: HashMap<usize, StreamHandle>,

    stream_task_set: JoinSet<std::result::Result<(), StreamError>>,
    egui_ctx: egui::Context,
    tokio_egui_bridge: TokioEguiBridge,
    last_stream_id: usize,
    to_frontend: UnboundedSender<TopLevelBackendEvent>,
    from_frontend: UnboundedReceiver<TopLevelFrontendEvent>,
}

impl Backend {
    pub async fn init(
        egui_ctx: egui::Context,
        from_frontend: UnboundedReceiver<TopLevelFrontendEvent>,
        to_frontend: UnboundedSender<TopLevelBackendEvent>,
        tokio_egui_bridge: TokioEguiBridge,
    ) -> Self {
        Backend {
            egui_ctx,
            streams: HashMap::default(),
            tokio_egui_bridge: tokio_egui_bridge.clone(),
            last_stream_id: 0,
            to_frontend,
            from_frontend,
            pending_streams: HashMap::default(),
            stream_task_set: JoinSet::new(),
        }
    }
    pub async fn move_to_ephemeral_port(&mut self, mut stream: TcpStream) -> Result<TcpStream> {
        // Create ephemeral listener
        let ephemeral_listener = TcpListener::bind("127.0.0.1:0").await?;
        let ephemeral_port = ephemeral_listener.local_addr()?.port();

        info!("Created ephemeral port: {}", ephemeral_port);

        // Send ephemeral port to client
        let msg = format!("PORT {ephemeral_port}\n");
        stream.write_all(msg.as_bytes()).await?;

        let (new_stream, _addr) = ephemeral_listener
            .accept()
            .await
            .wrap_err("Failed to accept connection on ephemeral port")?;

        Ok(new_stream)
    }

    pub async fn run(&mut self) {
        let address = "127.0.0.1:8080".to_string();
        let listener = TcpListener::bind(&address)
            .await
            .expect("Failed to bind TCP listener");
        info!("TCP server listening on {}", address);

        loop {
            tokio::select! {
                r = listener.accept() => {
                    match r {
                        Ok((tcp_stream, addr)) => {
                            info!("Accepted connection from {}", addr);
                            let tcp_stream = self.move_to_ephemeral_port(tcp_stream).await.expect("Failed to move to ephemeral port");
                            self.last_stream_id += 1;
                            let stream_id = self.last_stream_id;
                            self.pending_streams.insert(stream_id, tcp_stream);
                            self.to_frontend.send(TopLevelBackendEvent::NewPendingStream{ stream_id, addr }).expect("Failed to send new pending stream event");
                        }
                        Err(e) => {
                            error!("Failed to accept TCP connection: {:?}", e);
                        }
                    }
                },
                Some(join_handle) = self.stream_task_set.join_next() => {
                    match join_handle {
                        Ok(Ok(())) => {
                            info!("Stream task completed successfully");
                        }
                        Ok(Err(e)) => {
                            let stream_id = e.stream_id;
                            error!("Stream task failed with error: {:?}", e);
                            if let Some(stream_handle) = self.streams.remove(&stream_id) {
                                stream_handle.terminate();
                            }
                            else {
                                trace!("Stream with ID {} was not found in the streams map", stream_id);
                                // Stream was already removed, this is fine since the error from
                                // both data and tcp tasks could be the cause
                            }
                        }

                        Err(e) => {
                            error!("Stream task panicked: {:?}", e);
                        }
                    }
                },
                event = self.from_frontend.recv() => {
                    match event {
                        Some(event) => self
                            .handle_top_level_frontend_event(event)
                            .await
                            .expect("Failed to handle frontend event"),
                        None => {
                            info!("Frontend event channel closed, shutting down backend");
                            break;
                        }
                    }
                },
               _ = self.tokio_egui_bridge.cancelled_fut() => {
                    warn!("Tokio task cancelled, shutting down...");
                    break; // important!
                },
            };
        }

        debug!("Backend event loop exited");
    }

    async fn handle_top_level_frontend_event(
        &mut self,
        event: TopLevelFrontendEvent,
    ) -> Result<()> {
        debug!("Handling top-level frontend event: {:?}", event);
        match event {
            TopLevelFrontendEvent::OpenStream {
                stream_id,
                on_pane_id,
            } => {
                trace!("Opening stream for pane ID: {:?}", on_pane_id);
                dbg!(self.pending_streams.get(&stream_id));
                let tcp_stream = self
                    .pending_streams
                    .remove(&stream_id)
                    .expect("No pending stream found for pane ID");

                // TODO
                //let (stream_ctrl_tx, stream_ctrl_rx) = unbounded_channel::<StreamCtrl>();
                let (frontend_comm_side, backend_comm_side) =
                    FrontendBackendComm::for_stream(stream_id, on_pane_id);

                self.to_frontend
                    .send(TopLevelBackendEvent::StreamStarted(frontend_comm_side))?;

                Stream::new(
                    stream_id,
                    tcp_stream,
                    on_pane_id,
                    backend_comm_side,
                    self.tokio_egui_bridge.clone(),
                )
                .run()
                .await?;
            }
            TopLevelFrontendEvent::CloseStream {
                stream_id,
                on_pane_id,
            } => {
                trace!("Closing stream for pane ID: {:?}", on_pane_id);
                let stream = self
                    .streams
                    .get_mut(&stream_id)
                    .expect("No stream found for pane ID");
                info!("Stopping stream with ID {}", stream_id);
                stream.terminate();
                self.streams
                    .remove(&stream_id)
                    .expect("Stream should be in the map");
            }
        }
        Ok(())
    }
}

pub struct LogAppendBuf<T> {
    phantom: std::marker::PhantomData<T>,
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
    pub dropped_writer: bool,
}

impl<T> LogAppendBuf<T> {
    pub fn split() -> (LogAppendBufWriter<T>, LogAppendBufReader<T>) {
        let inner = Arc::new(Mutex::new(VecDeque::new()));
        (
            LogAppendBufWriter {
                inner: inner.clone(),
            },
            LogAppendBufReader {
                inner,
                dropped_writer: false,
            },
        )
    }
}

impl<T> LogAppendBufWriter<T> {
    pub fn push(&self, item: T) {
        let mut guard = self.inner.lock();
        guard.push_back(item);
    }

    #[allow(unused)]
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

type LogCollection = VecDeque<Arc<DashboardEvent>>;

#[derive(Debug, Clone, Default)]
pub struct DataToDisplay {
    // Vector of references to the logs that match the current filter
    // The logs are not actually stored here, so clone is cheap
    pub filtered_logs: LogCollection,
    pub log_counts: LogCounts,

    pub oculus_memory_usage_mb: f64,
}

#[derive(Debug, Clone, Default, Copy)]
pub struct LogCounts {
    pub total: usize,
    pub trace: usize,
    pub debug: usize,
    pub info: usize,
    pub warn: usize,
    pub error: usize,
}

impl LogCounts {
    fn from_logs(logs: &LogCollection) -> Self {
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

    fn reset(&mut self) {
        *self = LogCounts::default();
    }
}

struct MemoryTracker {
    sys: System,
    pid: sysinfo::Pid,
}

impl Default for MemoryTracker {
    fn default() -> Self {
        let sys = System::new();
        let pid = sysinfo::get_current_pid().expect("Bug: Failed to get current PID");
        Self { sys, pid }
    }
}

impl MemoryTracker {
    fn refresh_processes(&mut self) {
        let _r = self
            .sys
            .refresh_processes(sysinfo::ProcessesToUpdate::Some(&[self.pid]), true);
    }

    fn calc_memory_usage_mb(&mut self) -> f64 {
        self.refresh_processes();
        if let Some(proc) = self.sys.process(self.pid) {
            // memory is in Bytes → convert to MB with decimals
            proc.memory() as f64 / (1024.0 * 1024.0)
        } else {
            0.0
        }
    }
}

// Should be fast
fn contains_case_insensitive(haystack: &str, needle: &str) -> bool {
    let needle_len = needle.len();
    if needle_len == 0 {
        return true;
    }

    haystack
        .as_bytes()
        .windows(needle_len)
        .any(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}
