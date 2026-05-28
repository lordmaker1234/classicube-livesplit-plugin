use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use futures_util::{SinkExt, StreamExt};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::broadcast,
    time::{MissedTickBehavior, interval, sleep},
};
use tokio_tungstenite::{WebSocketStream, connect_async, tungstenite};
use tracing::{debug, info, trace, warn};
use tungstenite::Message;

use crate::{chat_print_async, plugin::livesplit::protocol::Command};

const BACKOFF_INITIAL: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(5);

pub async fn run(
    url: String,
    mut command_rx: broadcast::Receiver<Command>,
    connected: Arc<AtomicBool>,
) {
    let mut backoff = BACKOFF_INITIAL;
    loop {
        info!("dialing LiveSplit at {url}");
        match connect_async(&url).await {
            Ok((ws, _resp)) => {
                info!("connected to LiveSplit at {url}");
                chat_print_async(format!("&aLiveSplit: connected to {url}"));
                connected.store(true, Ordering::Relaxed);
                backoff = BACKOFF_INITIAL;

                run_session(ws, &mut command_rx).await;

                connected.store(false, Ordering::Relaxed);
                info!("disconnected from LiveSplit at {url}; will reconnect");
                chat_print_async(format!("&eLiveSplit: disconnected from {url}"));
            }
            Err(e) => {
                debug!("dial failed for {url}: {e}");
            }
        }
        sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

async fn run_session<S>(mut ws: WebSocketStream<S>, command_rx: &mut broadcast::Receiver<Command>)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut ping = interval(Duration::from_secs(10));
    ping.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            cmd = command_rx.recv() => {
                match cmd {
                    Ok(cmd) => {
                        if send_command(&mut ws, &cmd).await.is_err() {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("client lagged; dropped {n} commands");
                    }
                }
            }

            msg = ws.next() => {
                match msg {
                    Some(Ok(Message::Text(t))) => trace!(text = %t.as_str(), "ls inbound"),
                    Some(Ok(Message::Binary(_))) => trace!("ls inbound binary"),
                    Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
                    Some(Ok(Message::Close(c))) => {
                        debug!(?c, "LiveSplit peer closed");
                        return;
                    }
                    Some(Err(e)) => {
                        debug!("WS read error: {e}");
                        return;
                    }
                    None => {
                        debug!("LiveSplit stream ended");
                        return;
                    }
                }
            }

            _ = ping.tick() => {
                if send_command(&mut ws, &Command::Ping).await.is_err() {
                    return;
                }
            }
        }
    }
}

async fn send_command<S>(
    ws: &mut WebSocketStream<S>,
    cmd: &Command,
) -> Result<(), tungstenite::Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let json = serde_json::to_string(cmd).expect("Command serialization is infallible");
    trace!(?cmd, %json, "ls outbound");
    ws.send(Message::text(json)).await
}
