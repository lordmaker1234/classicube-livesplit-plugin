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
use tokio::{sync::mpsc, task::JoinHandle};
use tracing::debug;

pub use crate::plugin::livesplit::protocol::Command;
use crate::plugin::module::Module;

thread_local! {
    static SENDER: RefCell<Option<mpsc::UnboundedSender<Command>>> = const { RefCell::new(None) };
    static CONNECTED: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

pub struct LiveSplitModule {
    handle: JoinHandle<()>,
}

impl LiveSplitModule {
    pub fn init() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let connected = Arc::new(AtomicBool::new(false));

        SENDER.with_borrow_mut(|s| *s = Some(tx));
        CONNECTED.with_borrow_mut(|c| *c = Some(connected.clone()));

        let handle = async_manager::spawn(server::run(rx, connected));
        Self { handle }
    }
}

impl Module for LiveSplitModule {
    fn free(&mut self) {
        SENDER.with_borrow_mut(|s| {
            let _ = s.take();
        });
        CONNECTED.with_borrow_mut(|c| {
            let _ = c.take();
        });
        self.handle.abort();
        debug!("LiveSplit module freed; server task aborted");
    }
}

/// Fire-and-forget send of a LiveSplit command. Silently no-ops if no client
/// is connected (or if the plugin is mid-reload).
pub fn send(cmd: Command) {
    SENDER.with_borrow(|s| {
        if let Some(s) = s {
            let _ = s.send(cmd);
        }
    });
}

/// Whether a LiveSplit WebSocket client is currently connected.
pub fn is_connected() -> bool {
    CONNECTED.with_borrow(|c| {
        c.as_ref()
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(false)
    })
}
