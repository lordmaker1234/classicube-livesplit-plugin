#[cfg(windows)]
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

#[cfg(windows)]
pub use crate::plugin::livesplit::client::PIPE_NAME as CLIENT_PIPE_NAME;
pub use crate::plugin::livesplit::{
    protocol::{Command, TimerEvent},
    server::BIND_ADDR as SERVER_BIND_ADDR,
};
use crate::plugin::module::Module;

const BROADCAST_CAPACITY: usize = 64;

thread_local! {
    static CMD_TX: RefCell<Option<broadcast::Sender<Command>>> = const { RefCell::new(None) };
    static SERVER_CONNECTED: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
    /// True while the built-in `TimerModule` is active. Included in
    /// `any_connected()` so the plugin works fully offline.
    static TIMER_CONNECTED: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

#[cfg(windows)]
thread_local! {
    static CLIENT_CONNECTED: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

pub struct LiveSplitModule {
    server_handle: JoinHandle<()>,
    #[cfg(windows)]
    client_handle: JoinHandle<()>,
    timer_handle: JoinHandle<()>,
}

impl LiveSplitModule {
    pub fn init() -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        let server_rx = tx.subscribe();

        let server_connected = Arc::new(AtomicBool::new(false));
        SERVER_CONNECTED.with_borrow_mut(|c| *c = Some(server_connected.clone()));
        let server_handle = async_manager::spawn(server::run(server_rx, server_connected));

        #[cfg(windows)]
        let client_handle = {
            let client_rx = tx.subscribe();
            let client_connected = Arc::new(AtomicBool::new(false));
            CLIENT_CONNECTED.with_borrow_mut(|c| *c = Some(client_connected.clone()));
            async_manager::spawn(client::run(client_rx, client_connected))
        };

        // Third subscriber: forward every command to the main-thread built-in
        // timer state machine via spawn_on_main_thread (the same hop pattern
        // the LSO read loop uses for timer events in server.rs).
        let timer_rx = tx.subscribe();
        let timer_connected = Arc::new(AtomicBool::new(true));
        TIMER_CONNECTED.with_borrow_mut(|c| *c = Some(timer_connected));
        let timer_handle = async_manager::spawn(timer_forward_loop(timer_rx));

        CMD_TX.with_borrow_mut(|c| *c = Some(tx));

        Self {
            server_handle,
            #[cfg(windows)]
            client_handle,
            timer_handle,
        }
    }
}

async fn timer_forward_loop(mut rx: broadcast::Receiver<Command>) {
    loop {
        match rx.recv().await {
            Ok(cmd) => {
                async_manager::spawn_on_main_thread(async move {
                    crate::plugin::timer::apply_command(cmd);
                });
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!(n, "built-in timer lagged; some commands were missed");
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

impl Module for LiveSplitModule {
    fn free(&mut self) {
        self.timer_handle.abort();
        #[cfg(windows)]
        self.client_handle.abort();
        self.server_handle.abort();
        CMD_TX.with_borrow_mut(|c| {
            c.take();
        });
        SERVER_CONNECTED.with_borrow_mut(|c| {
            c.take();
        });
        TIMER_CONNECTED.with_borrow_mut(|c| {
            c.take();
        });
        #[cfg(windows)]
        CLIENT_CONNECTED.with_borrow_mut(|c| {
            c.take();
        });
        debug!("LiveSplit module freed; tasks aborted");
    }
}

/// Fire-and-forget broadcast of a LiveSplit command to whichever timers
/// are currently connected (LSO via the server side, LiveSplit desktop via
/// the named-pipe client on Windows, built-in timer, or any combination).
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

#[cfg(windows)]
pub fn client_connected() -> bool {
    CLIENT_CONNECTED.with_borrow(|c| {
        c.as_ref()
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(false)
    })
}

pub fn timer_connected() -> bool {
    TIMER_CONNECTED.with_borrow(|c| {
        c.as_ref()
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(false)
    })
}

pub fn external_connected() -> bool {
    if server_connected() {
        return true;
    }
    #[cfg(windows)]
    if client_connected() {
        return true;
    }
    false
}

pub fn any_connected() -> bool {
    if external_connected() {
        return true;
    }
    if timer_connected() {
        return true;
    }
    false
}
