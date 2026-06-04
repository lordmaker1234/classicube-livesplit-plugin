pub mod path;
pub mod payload;
pub mod read;
pub mod write;

use std::{cell::RefCell, path::PathBuf};

use classicube_helpers::{async_manager, tick::TickEventHandler};
use classicube_sys::Server;
use tracing::debug;

use crate::{
    chat::chat_print_async,
    chat_print,
    plugin::{
        editor,
        module::Module,
        splits::{self, geometry::Track},
    },
};

thread_local! {
    /// Path of the `.lss` file the currently loaded track was read from,
    /// relative to the working dir (e.g.
    /// `plugins/livesplit/<server>/<map>/<category>-vN.lss`). `None` when
    /// the track came from the chat protocol / the `loadtest` fixture, or
    /// when no track is loaded. Read by `/client LiveSplit open` to reveal
    /// the exact file in the OS file manager.
    static LOADED_LSS_PATH: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Remember the file the just-loaded track came from. Called by the
/// disk-read paths (`read::try_autoload` / `read::load_command`) *after*
/// a successful `splits::load_track`, so it overwrites the clear that
/// `on_track_loaded` performed synchronously inside that same call.
pub(super) fn set_loaded_path(path: PathBuf) {
    LOADED_LSS_PATH.with_borrow_mut(|p| *p = Some(path));
}

fn loaded_path() -> Option<PathBuf> {
    LOADED_LSS_PATH.with_borrow(Clone::clone)
}

fn clear_loaded_path() {
    LOADED_LSS_PATH.with_borrow_mut(|p| *p = None);
}

pub struct LssStorageModule {
    // Owned for its Drop side-effect: TickEventHandler::Drop unregisters
    // the closure from the helpers crate's tick callback list.
    _tick: TickEventHandler,
}

impl LssStorageModule {
    pub fn init() -> Self {
        splits::set_load_callback(on_track_loaded);

        // Autoload is driven from the tick rather than `on_new_map_loaded`
        // for the same reason `splits::observe_map` is: at the moment
        // `MapLoaded` fires on multiplayer, `World.Name` has been zeroed
        // by `World_SetNewMap` and the server hasn't yet pushed the
        // updated `"On <map>"` tab-list group, so the name resolved at
        // the event would be the *previous* map's. Polling until both
        // signals settle gives us the correct destination directory.
        let mut tick = TickEventHandler::new();
        let mut last_seen_map: Option<String> = None;
        tick.on(move |_event| {
            let cur = splits::current_map();
            if last_seen_map == cur {
                return;
            }
            last_seen_map.clone_from(&cur);
            let Some(map) = cur else {
                return;
            };
            if splits::run_in_progress() {
                debug!("autoload skipped: run in progress");
                return;
            }
            if editor::is_enabled() && splits::track_includes_map(&map) {
                debug!("autoload skipped: editing a track that includes this map");
                return;
            }
            let Some(server) = current_server_display() else {
                return;
            };
            async_manager::spawn(read::try_autoload(server, map));
        });

        Self { _tick: tick }
    }
}

impl Module for LssStorageModule {
    fn free(&mut self) {
        splits::clear_load_callback();
        clear_loaded_path();
        debug!("LssStorageModule freed; load callback cleared");
    }
}

fn on_track_loaded(track: &Track, starting_map: Option<&str>) {
    // A track just loaded from *some* source. Forget any remembered
    // `.lss` path: the disk-read paths re-set it right after this returns
    // (`set_loaded_path`), while chat / fixture loads leave it `None` so
    // `/client LiveSplit open` falls back to the track directory.
    clear_loaded_path();

    // A brand-new empty track (editor `edit new`) has nothing worth
    // persisting yet; skip the background autosave so we don't write a
    // 0-checkpoint `.lss`. The explicit `/client LiveSplit save` after
    // placing checkpoints is the first write.
    if track.checkpoints.is_empty() {
        return;
    }

    let Some(server) = current_server_display() else {
        debug!("save skipped: no server name");
        return;
    };
    let Some(map) = starting_map else {
        debug!("save skipped: no starting map");
        return;
    };
    let track = track.clone();
    let map = map.to_owned();
    // Background autosave: silent on the no-op (dedup) case.
    async_manager::spawn(write::save_track(track, server, map, false));
}

/// Write the currently loaded track to disk on demand
/// (`/client LiveSplit save`), reusing the autosave writer. Files the
/// track under `(server, starting_map)` -- the map the track is scoped
/// to, not the live world. Announces the no-op case so a manual save
/// with no changes is clean feedback rather than silence. Not gated on
/// `require_connected()` -- file I/O is local.
pub fn save_current_track() {
    let Some(track) = splits::current_track() else {
        chat_print("&cLiveSplit: no track loaded to save");
        return;
    };
    if track.checkpoints.is_empty() {
        chat_print("&eLiveSplit: nothing to save yet (track has no checkpoints)");
        return;
    }
    let Some(server) = current_server_display() else {
        chat_print("&cLiveSplit: cannot resolve server name to save under");
        return;
    };
    let Some(map) = splits::starting_map() else {
        chat_print("&cLiveSplit: no starting map for this track (load it on a map first)");
        return;
    };
    async_manager::spawn(write::save_track(track, server, map, true));
}

/// Load a track from disk on demand (`/client LiveSplit load [filename]`).
/// `None` loads the newest `.lss`; `Some(name)` a specific file from the
/// current `(server, map)` directory. Resolves `(server, map)` here on
/// the main thread (both read engine globals) and hands owned strings to
/// the disk-reading task. Not gated on `require_connected()` /
/// `run_in_progress()` -- the command is explicit.
pub fn load_track_command(filename: Option<String>) {
    let Some(server) = current_server_display() else {
        chat_print("&cLiveSplit: cannot resolve server name to load from");
        return;
    };
    let Some(map) = splits::current_map() else {
        chat_print("&cLiveSplit: no current map to load a track for");
        return;
    };
    async_manager::spawn(read::load_command(server, map, filename));
}

/// Open the OS file manager for the currently loaded track
/// (`/client LiveSplit open`). When the track was loaded from a `.lss`
/// file, reveals that exact file (selected) in its containing folder;
/// otherwise (chat / fixture load, or the file has since moved) opens
/// the track's `(server, starting_map)` directory. Path resolution runs
/// here on the main thread (engine globals + splits `STATE`); the actual
/// `opener` call is spawned to a task because the Linux DBus path can
/// block. Not gated on `require_connected()` -- this is local file state.
pub fn open_track_location() {
    if splits::current_track().is_none() {
        chat_print("&cLiveSplit: no track loaded");
        return;
    }

    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            chat_print(&format!("&cLiveSplit: cannot resolve working dir: {e}"));
            return;
        }
    };

    // Prefer revealing the exact file the current track was loaded from.
    // Build the absolute path via `current_dir().join` rather than
    // `fs::canonicalize`: on Windows the latter yields a `\\?\` UNC path
    // that `explorer /select,` mishandles (`opener` normalizes a plain
    // absolute path itself).
    if let Some(rel) = loaded_path() {
        let abs = cwd.join(rel);
        if abs.is_file() {
            chat_print(&format!("&aLiveSplit: opening {}", abs.display()));
            async_manager::spawn(async move {
                if let Err(e) = opener::reveal(&abs) {
                    chat_print_async(format!("&cLiveSplit: failed to open file manager: {e}"));
                }
            });
            return;
        }
    }

    // Fallback: open the track's directory. Files under
    // `(server, starting_map)` -- the same scope `/client LiveSplit save`
    // writes to (the chat-load autosave in `on_track_loaded` already
    // created it for a chat-sourced track).
    let Some(server) = current_server_display() else {
        chat_print("&cLiveSplit: cannot resolve server name");
        return;
    };
    let Some(map) = splits::starting_map() else {
        chat_print("&cLiveSplit: no starting map for this track");
        return;
    };
    let dir = cwd.join(path::track_dir(&server, &map));
    if !dir.is_dir() {
        chat_print("&cLiveSplit: track not saved to disk yet (run /client LiveSplit save)");
        return;
    }
    chat_print(&format!("&aLiveSplit: opening {}", dir.display()));
    async_manager::spawn(async move {
        if let Err(e) = opener::open(&dir) {
            chat_print_async(format!("&cLiveSplit: failed to open folder: {e}"));
        }
    });
}

/// Resolve the unsanitized display server name. Returns
/// `"singleplayer"` placeholder when in singleplayer mode (the wire
/// `Server.Name` is empty there). Color codes are left in place for
/// the writer's display-only consumer; the path sanitizer strips
/// them later when building filesystem paths.
fn current_server_display() -> Option<String> {
    // SAFETY: `Server` is the engine's `static mut _ServerConnectionData`.
    // We're called on the main game thread (via `on_track_loaded` from
    // `splits::load_track`, or via `on_new_map_loaded` dispatch). `&raw`
    // avoids creating an `&'static mut` ref (Rust 2024 `static_mut_refs`
    // lint). `cc_string`'s `Display` impl copies through the buffer.
    let server_ptr = &raw const Server;
    let is_sp = unsafe { (*server_ptr).IsSinglePlayer } != 0;
    if is_sp {
        return Some("singleplayer".to_owned());
    }
    let name = unsafe { (*server_ptr).Name.to_string() };
    if name.is_empty() { None } else { Some(name) }
}
