use classicube_helpers::{async_manager, chat};

pub fn chat_print(text: &str) {
    chat::print(text);
}

/// Schedule a `chat_print` on the game's main thread. Safe to call from
/// any tokio task; the message is forwarded via `async_manager`'s
/// tick-driven dispatcher.
pub fn chat_print_async(msg: String) {
    async_manager::spawn_on_main_thread(async move {
        chat::print(&msg);
    });
}
