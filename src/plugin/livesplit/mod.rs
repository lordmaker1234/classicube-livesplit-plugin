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
use tracing::{debug, warn};

pub use crate::plugin::livesplit::protocol::Command;
use crate::plugin::module::Module;

const BROADCAST_CAPACITY: usize = 64;

thread_local! {
    static CMD_TX: RefCell<Option<broadcast::Sender<Command>>> = const { RefCell::new(None) };
    static SERVER_CONNECTED: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
    static CLIENT_HANDLE: RefCell<Option<JoinHandle<()>>> = const { RefCell::new(None) };
    static CLIENT_URL: RefCell<Option<String>> = const { RefCell::new(None) };
    static CLIENT_CONNECTED: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

pub struct LiveSplitModule {
    server_handle: JoinHandle<()>,
}

impl LiveSplitModule {
    pub fn init() -> Self {
        let (tx, server_rx) = broadcast::channel(BROADCAST_CAPACITY);
        let server_connected = Arc::new(AtomicBool::new(false));

        CMD_TX.with_borrow_mut(|c| *c = Some(tx));
        SERVER_CONNECTED.with_borrow_mut(|c| *c = Some(server_connected.clone()));

        let server_handle = async_manager::spawn(server::run(server_rx, server_connected));
        Self { server_handle }
    }
}

impl Module for LiveSplitModule {
    fn free(&mut self) {
        if let Some(handle) = CLIENT_HANDLE.with_borrow_mut(|h| h.take()) {
            handle.abort();
        }
        CLIENT_URL.with_borrow_mut(|u| {
            u.take();
        });
        CLIENT_CONNECTED.with_borrow_mut(|c| {
            c.take();
        });
        CMD_TX.with_borrow_mut(|c| {
            c.take();
        });
        SERVER_CONNECTED.with_borrow_mut(|c| {
            c.take();
        });
        self.server_handle.abort();
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

pub fn client_target_url() -> Option<String> {
    CLIENT_URL.with_borrow(|u| u.clone())
}

pub fn client_connected() -> bool {
    CLIENT_CONNECTED.with_borrow(|c| {
        c.as_ref()
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(false)
    })
}

/// Start (or replace) the client task that dials `url`. The task
/// reconnects with exponential backoff until [`disconnect`] is called or
/// the plugin is freed.
pub fn connect(url: String) {
    disconnect();

    let rx = match CMD_TX.with_borrow(|c| c.as_ref().map(broadcast::Sender::subscribe)) {
        Some(rx) => rx,
        None => {
            warn!("livesplit::connect called while module is inactive");
            return;
        }
    };

    let connected = Arc::new(AtomicBool::new(false));
    CLIENT_URL.with_borrow_mut(|u| *u = Some(url.clone()));
    CLIENT_CONNECTED.with_borrow_mut(|c| *c = Some(connected.clone()));

    let handle = async_manager::spawn(client::run(url, rx, connected));
    CLIENT_HANDLE.with_borrow_mut(|h| *h = Some(handle));
}

pub fn disconnect() {
    if let Some(handle) = CLIENT_HANDLE.with_borrow_mut(|h| h.take()) {
        handle.abort();
    }
    CLIENT_URL.with_borrow_mut(|u| {
        u.take();
    });
    CLIENT_CONNECTED.with_borrow_mut(|c| {
        c.take();
    });
}
