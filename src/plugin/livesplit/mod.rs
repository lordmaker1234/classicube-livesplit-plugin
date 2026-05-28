mod client;
mod server;

pub mod protocol;

use std::{
    cell::RefCell,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use classicube_helpers::async_manager;
use tokio::{sync::broadcast, task::JoinHandle};
use tracing::debug;

pub use crate::plugin::livesplit::{protocol::Command, server::BIND_ADDR as SERVER_BIND_ADDR};
use crate::plugin::module::Module;

const BROADCAST_CAPACITY: usize = 64;

/// Where the client task dials. Matches the default LiveSplit desktop
/// server endpoint (`/livesplit` path is required by the C# server).
pub const CLIENT_TARGET_URL: &str = "ws://127.0.0.1:16834/livesplit";

thread_local! {
    static CMD_TX: RefCell<Option<broadcast::Sender<Command>>> = const { RefCell::new(None) };
    static SERVER_CONNECTED: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
    static CLIENT_CONNECTED: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

pub struct LiveSplitModule {
    server_handle: JoinHandle<()>,
    client_handle: JoinHandle<()>,
}

impl LiveSplitModule {
    pub fn init() -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        let server_rx = tx.subscribe();
        let client_rx = tx.subscribe();

        let server_connected = Arc::new(AtomicBool::new(false));
        let client_connected = Arc::new(AtomicBool::new(false));

        CMD_TX.with_borrow_mut(|c| *c = Some(tx));
        SERVER_CONNECTED.with_borrow_mut(|c| *c = Some(server_connected.clone()));
        CLIENT_CONNECTED.with_borrow_mut(|c| *c = Some(client_connected.clone()));

        let server_handle = async_manager::spawn(server::run(server_rx, server_connected));
        let client_handle = async_manager::spawn(client::run(
            CLIENT_TARGET_URL.to_string(),
            client_rx,
            client_connected,
        ));

        Self {
            server_handle,
            client_handle,
        }
    }
}

impl Module for LiveSplitModule {
    fn free(&mut self) {
        self.client_handle.abort();
        self.server_handle.abort();
        CMD_TX.with_borrow_mut(|c| {
            c.take();
        });
        SERVER_CONNECTED.with_borrow_mut(|c| {
            c.take();
        });
        CLIENT_CONNECTED.with_borrow_mut(|c| {
            c.take();
        });
        debug!("LiveSplit module freed; server + client tasks aborted");
    }
}

/// Fire-and-forget broadcast of a LiveSplit command to whichever timers
/// are currently connected (LSO via the server side, LiveSplit desktop via
/// the client side, or both). Silently no-ops if neither is connected.
pub fn send(cmd: Command) {
    CMD_TX.with_borrow(|c| {
        if let Some(c) = c {
            let _ = c.send(cmd);
        }
    });
}

pub fn server_connected() -> bool {
    SERVER_CONNECTED.with_borrow(|c| {
        c.as_ref()
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(false)
    })
}

pub fn client_connected() -> bool {
    CLIENT_CONNECTED.with_borrow(|c| {
        c.as_ref()
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(false)
    })
}
