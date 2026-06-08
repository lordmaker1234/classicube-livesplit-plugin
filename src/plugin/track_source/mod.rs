pub mod decode;
pub mod encode;

use std::cell::Cell;

use classicube_helpers::chat::ProtocolMessageHook;
use tracing::debug;

use crate::{
    chat_print,
    plugin::{
        module::Module,
        splits,
        track_source::decode::{FrameOutcome, feed_chat_line},
    },
};

// Set on `FrameOutcome::Loaded(_)`; cleared on `on_new_map_loaded`.
// While set, a second `LS title …` in the same map is refused with a
// chat-warn instead of triggering another `splits::load_track` + write
// task — protects against re-broadcast spam from servers that
// announce per-message-block. Mid-track frames (`Buffered`) and
// `ParseError`s don't flip the latch, so a typo'd title can still be
// retried.
thread_local!(
    static LOADED_THIS_MAP: Cell<bool> = const { Cell::new(false) };
);

/// Trampoline callback for `ProtocolMessageHook`. Receives the text of
/// `MSG_TYPE_NORMAL` messages (the helper filters + extracts). Returns
/// `true` to suppress (hide) the line, `false` to let it render.
fn handle_chat_line(text: &str) -> bool {
    match feed_chat_line(text) {
        FrameOutcome::NotOurs => false, // render
        FrameOutcome::ParseError(msg) => {
            chat_print(&format!("&cLiveSplit: malformed track frame: {msg}"));
            // Fall through so the raw line still renders.
            false
        }
        FrameOutcome::Buffered => true, // mid-track frame; suppress
        FrameOutcome::Loaded(track) => {
            if LOADED_THIS_MAP.get() {
                chat_print("&eLiveSplit: ignoring re-broadcast; load a new map to allow updates");
                return true;
            }
            debug!(
                name = track.name,
                checkpoints = track.checkpoints.len(),
                "received chat-protocol track"
            );
            if splits::load_track(track, "chat") {
                LOADED_THIS_MAP.set(true);
                true // suppress
            } else {
                // load_track returned false: plugin mid-teardown.
                // Fall through so the line renders normally rather than
                // silently disappearing.
                false
            }
        }
    }
}

/// Reset the data thread-locals to their initial values: the streaming
/// decoder back to `Idle` and the re-broadcast latch off. Shared by
/// `free()` (teardown) and `reset()` (disconnect / local-map-load clean
/// slate). The message-handler hook is a resource owned by the
/// `ProtocolMessageHook` handle in `TrackSourceModule`, torn down via
/// `Drop` in `free()`.
fn invalidate() {
    decode::reset_state();
    LOADED_THIS_MAP.set(false);
}

pub struct TrackSourceModule {
    hook: Option<ProtocolMessageHook>,
}

impl TrackSourceModule {
    pub fn init() -> Self {
        // install() returns None in singleplayer (no Protocol layer), so
        // the SP gate lives inside the helper -- no Server check here.
        Self {
            hook: ProtocolMessageHook::install(handle_chat_line),
        }
    }
}

impl Module for TrackSourceModule {
    fn free(&mut self) {
        // Drop uninstalls (if we're on top of the chain) and clears the
        // callback so a future reload's install() doesn't hit the
        // double-install assert.
        self.hook = None;
        invalidate();
    }

    fn reset(&mut self) {
        // ClassiCube wipes Protocol.Handlers on every Game_Reset /
        // disconnect (Protocol.OnReset: Mem_Set + restore default).
        // reinstall() re-hooks after the wipe; it's idempotent (no-op if
        // we're already on top) and a no-op in singleplayer.
        if let Some(hook) = &self.hook {
            hook.reinstall();
        }
        invalidate();
    }

    fn on_new_map_loaded(&mut self) {
        LOADED_THIS_MAP.set(false);
    }
}
