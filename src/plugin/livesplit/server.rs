use std::{
    future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use futures_util::{SinkExt, StreamExt};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::broadcast,
    time::{MissedTickBehavior, interval},
};
use tokio_tungstenite::{WebSocketStream, accept_async, tungstenite};
use tracing::{debug, error, info, trace, warn};
use tungstenite::Message;

use crate::{chat_print_async, plugin::livesplit::protocol::Command};

pub const BIND_ADDR: &str = "127.0.0.1:16833";

pub async fn run(mut command_rx: broadcast::Receiver<Command>, connected: Arc<AtomicBool>) {
    let listener = match TcpListener::bind(BIND_ADDR).await {
        Ok(l) => {
            info!("LiveSplit WS server listening on ws://{BIND_ADDR}");
            l
        }
        Err(e) => {
            error!("failed to bind LiveSplit WS server on {BIND_ADDR}: {e}");
            chat_print_async(format!(
                "&cLiveSplit: failed to bind server on {BIND_ADDR}: {e}"
            ));
            return;
        }
    };

    let mut ws: Option<WebSocketStream<TcpStream>> = None;
    let mut ping = interval(Duration::from_secs(10));
    ping.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            accept = listener.accept() => {
                let (stream, peer) = match accept {
                    Ok(p) => p,
                    Err(e) => { warn!("accept() error: {e}"); continue; }
                };
                match accept_async(stream).await {
                    Ok(new_ws) => {
                        if let Some(mut old) = ws.replace(new_ws) {
                            let _ = old.close(None).await;
                            debug!("closed previous LiveSplit client (replaced by {peer})");
                            chat_print_async(
                                "&eLiveSplit: previous server client kicked".to_string(),
                            );
                        }
                        connected.store(true, Ordering::Relaxed);
                        info!("LiveSplit client connected from {peer}");
                        chat_print_async(format!("&aLiveSplit: server client connected ({peer})"));
                    }
                    Err(e) => warn!("WS handshake failed from {peer}: {e}"),
                }
            }

            cmd = command_rx.recv() => {
                let cmd = match cmd {
                    Ok(cmd) => cmd,
                    Err(broadcast::error::RecvError::Closed) => {
                        debug!("command channel closed; LiveSplit server shutting down");
                        if let Some(mut old) = ws.take() {
                            let _ = old.close(None).await;
                        }
                        connected.store(false, Ordering::Relaxed);
                        return;
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("server lagged; dropped {n} commands");
                        continue;
                    }
                };
                if !send_command(&mut ws, &cmd).await {
                    connected.store(false, Ordering::Relaxed);
                    chat_print_async("&eLiveSplit: server client disconnected".to_string());
                }
            }

            msg = next_message(&mut ws), if ws.is_some() => {
                match msg {
                    Some(Ok(Message::Text(t))) => trace!(text = %t.as_str(), "ls inbound"),
                    Some(Ok(Message::Binary(_))) => trace!("ls inbound binary"),
                    Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
                    Some(Ok(Message::Close(c))) => {
                        debug!(?c, "LiveSplit client closed");
                        ws = None;
                        connected.store(false, Ordering::Relaxed);
                        chat_print_async("&eLiveSplit: server client disconnected".to_string());
                    }
                    Some(Err(e)) => {
                        debug!("WS read error; dropping client: {e}");
                        ws = None;
                        connected.store(false, Ordering::Relaxed);
                        chat_print_async("&eLiveSplit: server client disconnected".to_string());
                    }
                    None => {
                        debug!("LiveSplit client stream ended");
                        ws = None;
                        connected.store(false, Ordering::Relaxed);
                        chat_print_async("&eLiveSplit: server client disconnected".to_string());
                    }
                }
            }

            _ = ping.tick() => {
                if ws.is_some() && !send_command(&mut ws, &Command::Ping).await {
                    connected.store(false, Ordering::Relaxed);
                    chat_print_async("&eLiveSplit: server client disconnected".to_string());
                }
            }
        }
    }
}

/// Serialize and send `cmd` to the current client. Returns `false` if there
/// was no client or the send failed (and the client was dropped).
async fn send_command(ws: &mut Option<WebSocketStream<TcpStream>>, cmd: &Command) -> bool {
    let Some(w) = ws.as_mut() else {
        trace!(?cmd, "ls outbound dropped (no client)");
        return false;
    };
    let json = serde_json::to_string(cmd).expect("Command serialization is infallible");
    trace!(?cmd, %json, "ls outbound");
    if let Err(e) = w.send(Message::text(json)).await {
        debug!("WS send failed; dropping client: {e}");
        *ws = None;
        return false;
    }
    true
}

async fn next_message(
    ws: &mut Option<WebSocketStream<TcpStream>>,
) -> Option<Result<Message, tungstenite::Error>> {
    match ws.as_mut() {
        Some(w) => w.next().await,
        None => future::pending().await,
    }
}
