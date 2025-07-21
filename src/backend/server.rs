//! Raw TCP handling - simple enough to hand roll it without axum

use crate::prelude::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{JoinHandle, JoinSet};

pub struct TcpServer {
    address: String,
    tasks: JoinSet<Result<()>>,
}

impl TcpServer {
    pub fn new(address: String) -> Self {
        TcpServer {
            address,
            tasks: JoinSet::new(),
        }
    }

    pub fn start(mut self) -> JoinHandle<Result<()>> {
        tokio::spawn(async move {
            // Bind to port 8080
            let listener = TcpListener::bind(&self.address).await?;
            info!("TCP server listening on {}", self.address);

            loop {
                tokio::select! {
                    r = listener.accept() => {
                        match r {
                            Err(e) => {
                                error!("Error accepting connection: {:?}", e);
                                continue;
                            },
                            Ok((stream, addr)) => {
                                info!("Accepted connection from {}", addr);
                                self.tasks.spawn(async move {
                                     TcpServer::handle_main_connection(stream).await?;
                                    Ok(())
                                });
                            },

                        }
                    }
                    Some(r) = self.tasks.join_next() => {
                        match r {
                            Ok(Ok(())) => {
                                info!("Task completed successfully");
                            },
                            Ok(Err(e)) => {
                                error!("Task failed with error: {:?}", e);
                            },
                            Err(e) => {
                                error!("Task panicked: {:?}", e);
                            }
                        }
                    }
                }
            }
        })
    }

    async fn handle_main_connection(mut stream: TcpStream) -> Result<()> {
        // Create ephemeral listener
        let ephemeral_listener = TcpListener::bind("0.0.0.0:0").await?;
        let ephemeral_port = ephemeral_listener.local_addr()?.port();

        println!("Created ephemeral port: {}", ephemeral_port);

        // Send ephemeral port to client
        let msg = format!("{}\n", ephemeral_port);
        stream.write_all(msg.as_bytes()).await?;

        // Handle incoming connection on ephemeral port
        tokio::spawn(async move {
            if let Err(e) = TcpServer::handle_ephemeral_connection(ephemeral_listener).await {
                eprintln!("Error on ephemeral handler: {:?}", e);
            }
        });

        Ok(())
    }

    async fn handle_ephemeral_connection(listener: TcpListener) -> Result<()> {
        let (mut stream, addr) = listener.accept().await?;
        println!("Ephemeral connection from {}", addr);

        let mut buf = [0u8; 1024];
        let n = stream.read(&mut buf).await?;

        if n > 0 {
            println!("Received: {:?}", &buf[..n]);
            stream.write_all(&buf[..n]).await?;
        }

        Ok(())
    }
}
