pub mod decode;
pub mod encode;

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
            debug!(
                name = track.name,
                checkpoints = track.checkpoints.len(),
                "received chat-protocol track"
            );
            if splits::load_track(track, "chat") {
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
        // Reset the streaming decoder back to `Idle` so a partial frame
        // doesn't survive teardown.
        decode::reset_state();
    }

    fn reset(&mut self) {
        // ClassiCube wipes Protocol.Handlers on every Game_Reset /
        // disconnect (Protocol.OnReset: Mem_Set + restore default).
        // reinstall() re-hooks after the wipe; it's idempotent (no-op if
        // we're already on top) and a no-op in singleplayer.
        if let Some(hook) = &self.hook {
            hook.reinstall();
        }
        // Clean slate: drop any half-decoded frame.
        decode::reset_state();
    }
}
