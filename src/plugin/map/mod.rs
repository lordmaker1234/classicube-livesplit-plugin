//! Single map-change observer plus the engine-global readers everyone
//! needs to answer "what map / server are we on".
//!
//! Before this module existed, three places independently polled the
//! "settled map name" each tick -- `splits::observe_map`, the lss-storage
//! autoload tick, and the editor's `disable_if_left_track` -- each with its
//! own latch, and the engine-global readers (`World.Name` / tab-list group,
//! `Server.Name` / `Server.IsSinglePlayer`) were scattered across `splits`
//! and `lss_storage`. `MapModule` owns **one** `TickEventHandler` and
//! **one** edge latch; on a settled-map transition it fires a fixed set of
//! callbacks. The readers live here as free fns so every consumer reads the
//! engine through one place.
//!
//! ## Callback fire order
//!
//! On a settled-map edge the tick fires **splits -> autoload**:
//! - `splits` first so its run engine advances `next_index` (via
//!   `observe_map` -> `step_on_map_loaded`) before...
//! - `autoload` reads `run_in_progress()` / `track_includes_map()`.
//!
//! The slots are fixed (not a `Vec`) because this order is load-bearing and
//! the subscriber set is a known two.
//!
//! ## Tick ordering vs `SplitsModule`
//!
//! `MapModule` is constructed **before** `SplitsModule` (see the
//! `children()` ordering in `plugin::mod`). The helpers crate fires tick
//! callbacks in registration (construction) order, so the map-edge tick
//! (driving `step_on_map_loaded` through the splits callback) runs before
//! `SplitsModule`'s own AABB-`step` tick within the same frame -- preserving
//! the former back-to-back `observe_map` then `step`. Reverse-dispatch
//! teardown frees `editor` / `lss_storage` / `splits` (each clearing its
//! callback slot) before `map`, so the map tick can never call a half-freed
//! subscriber.

use std::cell::{Cell, RefCell};

use classicube_helpers::{
    entities::ENTITY_SELF_ID, tab_list::TabListEntry, tick::TickEventHandler,
};
use classicube_sys::{Server, World};
use tracing::debug;

use crate::plugin::module::Module;

/// A settled-map-change subscriber. Receives the new map name on the edge.
pub type MapCallback = fn(&str);

thread_local! {
    /// Last settled map name fired on. Drives the edge: a callback round
    /// runs only when the freshly-observed name differs from this. A `None`
    /// observation never clears or fires it (see [`observe`]).
    static LAST_SEEN_MAP: RefCell<Option<String>> = const { RefCell::new(None) };

    /// Fixed subscriber slots, invoked in declaration order on each edge
    /// (splits -> autoload). Registered by the respective modules at `init`,
    /// cleared at `free`.
    static SPLITS_CALLBACK: Cell<Option<MapCallback>> = const { Cell::new(None) };
    static AUTOLOAD_CALLBACK: Cell<Option<MapCallback>> = const { Cell::new(None) };
}

pub fn set_splits_callback(f: MapCallback) {
    SPLITS_CALLBACK.set(Some(f));
}
pub fn clear_splits_callback() {
    SPLITS_CALLBACK.set(None);
}

pub fn set_autoload_callback(f: MapCallback) {
    AUTOLOAD_CALLBACK.set(Some(f));
}
pub fn clear_autoload_callback() {
    AUTOLOAD_CALLBACK.set(None);
}

/// Snapshot the current map name. In singleplayer / file-loaded worlds the
/// engine populates `World.Name` directly. The classic / CPE network
/// protocol carries no map-name packet, so on multiplayer `World.Name` is
/// always empty; MCGalaxy and compatible servers instead put `"On <map>"`
/// in the local player's tab-list group (the section header the tab UI
/// groups players by). Read that and strip the prefix. Returns `None` if
/// neither source resolves a non-empty name.
pub fn current_map() -> Option<String> {
    // SAFETY: `World` is the engine's `static mut _WorldData`. We're called
    // on the main thread (the tick callback, or accessor free fns invoked
    // from chat/command handlers). `cc_string`'s `Display` impl copies
    // through the buffer pointer into an owned `String`. `&raw const` avoids
    // creating an `&'static mut` (the Rust 2024 `static_mut_refs` lint).
    let world_ptr = &raw const World;
    let name = unsafe { (*world_ptr).Name.to_string() };
    if !name.is_empty() {
        return Some(name);
    }

    let entry = unsafe { TabListEntry::from_id(ENTITY_SELF_ID) }?;
    let group = entry.get_group();
    let map = group.strip_prefix("On ")?.trim();
    if map.is_empty() {
        None
    } else {
        Some(map.to_owned())
    }
}

/// Whether the client is in singleplayer (the engine sets
/// `Server.IsSinglePlayer`; the wire `Server.Name` is empty there).
pub fn is_singleplayer() -> bool {
    // SAFETY: `Server` is the engine's `static mut _ServerConnectionData`;
    // main-thread access. `&raw const` avoids an `&'static mut` ref.
    let server_ptr = &raw const Server;
    unsafe { (*server_ptr).IsSinglePlayer != 0 }
}

/// Resolve the unsanitized display server name. Returns the
/// `"singleplayer"` placeholder in singleplayer mode. Color codes are left
/// in place for display-only consumers; the path sanitizer strips them
/// later when building filesystem paths.
pub fn current_server_display() -> Option<String> {
    if is_singleplayer() {
        return Some("singleplayer".to_owned());
    }
    // SAFETY: as in `is_singleplayer`; `cc_string`'s `Display` impl copies
    // through the buffer.
    let server_ptr = &raw const Server;
    let name = unsafe { (*server_ptr).Name.to_string() };
    if name.is_empty() { None } else { Some(name) }
}

/// Tick body: detect a settled-map edge and fan it out to the subscribers.
fn observe() {
    // None-ignore: a transient gap (e.g. `World.Name` zeroed before the
    // tab-list group catches up after a server-driven map change) must
    // never clear or fire the edge -- mirrors `observe_map`'s rule.
    // Re-evaluate when the real name arrives on a later tick.
    let Some(map) = current_map() else {
        return;
    };
    let changed = LAST_SEEN_MAP.with_borrow(|last| last.as_deref() != Some(map.as_str()));
    if !changed {
        return;
    }
    LAST_SEEN_MAP.with_borrow_mut(|last| *last = Some(map.clone()));

    // Fixed order: splits advances its cursor before autoload reads it.
    if let Some(f) = SPLITS_CALLBACK.get() {
        f(&map);
    }
    if let Some(f) = AUTOLOAD_CALLBACK.get() {
        f(&map);
    }
}

pub struct MapModule {
    // Owned for its Drop side-effect: TickEventHandler::Drop unregisters the
    // observe closure from the helpers crate's tick callback list.
    _tick: TickEventHandler,
}

impl MapModule {
    pub fn init() -> Self {
        let mut tick = TickEventHandler::new();
        tick.on(|_event| observe());
        Self { _tick: tick }
    }
}

impl Module for MapModule {
    fn free(&mut self) {
        // Subscribers cleared their slots already (reverse-dispatch frees
        // them before us); drop the latch so a fresh Init starts clean.
        LAST_SEEN_MAP.with_borrow_mut(|last| *last = None);
        debug!("MapModule freed; map latch cleared");
    }

    fn reset(&mut self) {
        // Disconnect / local-map-load: forget the last-seen map so the next
        // settled name re-fires the edge (re-autoloading the track for the
        // current map after a reconnect, even when the map name is
        // unchanged). The subscribers reset their own state separately.
        LAST_SEEN_MAP.with_borrow_mut(|last| *last = None);
    }
}
