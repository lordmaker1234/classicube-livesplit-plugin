//! In-world HUD overlay for the loaded track's checkpoint volumes.
//!
//! Draws each `Trigger::Aabb` checkpoint as a translucent colored cuboid
//! via ClassiCube's built-in `Selections_Add` / `Selections_Remove`
//! `CC_API`, color-coded by `CheckpointKind`. The engine re-renders its
//! selection list every frame, so the plugin never touches the GPU
//! directly -- the module is pure state management driven off the tick.
//!
//! Toggle with `/client LiveSplit show [on|off]` (default on). The
//! toggle is in-memory only: it resets to on every plugin Init.

use std::cell::{Cell, RefCell};

use classicube_helpers::tick::TickEventHandler;
use classicube_sys::{IVec3, PackedCol, PackedCol_Make, Selections_Add, Selections_Remove, Vec3};
use tracing::{debug, warn};

use crate::plugin::{
    module::Module,
    splits::{
        self,
        geometry::{Aabb, CheckpointKind},
    },
};

/// Private selection-id range for plugin-owned checkpoint boxes. Server
/// commands (`/zone`, `/measure`) allocate selection ids from 0 upward,
/// so a high base keeps us from clobbering them (and vice versa).
/// `200..=255` gives 56 simultaneous boxes; the engine's `SELECTIONS_MAX`
/// is 256.
const HUD_ID_BASE: u8 = 200;
/// Number of ids in `HUD_ID_BASE..=u8::MAX` (= 56).
const HUD_ID_COUNT: usize = (u8::MAX - HUD_ID_BASE) as usize + 1;

thread_local! {
    /// Whether the HUD is currently showing. Set by the
    /// `/client LiveSplit show` chat arm; read by the tick reconcile.
    static SHOW: Cell<bool> = const { Cell::new(true) };

    /// The scoped AABB set the currently-installed selections reflect:
    /// exactly the `(kind, aabb)` pairs we last drew, in draw order. The
    /// tick reconcile short-circuits while this equals the want-state,
    /// so steady-state cost is one walk + compare per tick. Caching the
    /// *filtered* set (not the whole `Track`) means a map crossing --
    /// which changes which AABBs are in scope -- naturally invalidates
    /// the cache. Reset to empty on map change / reset, because the
    /// engine wipes its own selection list on `OnNewMap` / `Reset` and
    /// our cache would otherwise wrongly suppress the re-add.
    static LAST_APPLIED: RefCell<Vec<(CheckpointKind, Aabb)>> = const { RefCell::new(Vec::new()) };

    /// Selection ids we currently have installed, so the next reconcile
    /// removes exactly what it added.
    static ACTIVE_IDS: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

pub fn set_show(show: bool) {
    SHOW.set(show);
}

/// Flip the HUD visibility and return the new state.
pub fn toggle_show() -> bool {
    let show = !SHOW.get();
    SHOW.set(show);
    show
}

pub struct HudModule {
    // Owned for its Drop side-effect: TickEventHandler::Drop unregisters
    // the reconcile closure from the helpers crate's tick callback list.
    _tick: TickEventHandler,
}

impl HudModule {
    pub fn init() -> Self {
        // In-memory only: every Init starts with the HUD on.
        SHOW.set(true);
        // Sweep our private id range in case a prior Init leaked
        // selections (crash / abnormal teardown). Removing an id that
        // isn't installed is a harmless no-op engine-side.
        clear_all_selections();

        let mut tick = TickEventHandler::new();
        tick.on(|_event| reconcile());
        Self { _tick: tick }
    }
}

impl Module for HudModule {
    fn free(&mut self) {
        clear_all_selections();
        LAST_APPLIED.with_borrow_mut(Vec::clear);
        debug!("HudModule freed; checkpoint selections cleared");
    }

    // The engine wipes its selection list on every `OnNewMap` / `Reset`
    // (`SelectionBox.c`'s OnReset zeroes `selections_count`). Our cached
    // "already applied" snapshot would then wrongly suppress the re-add,
    // so invalidate it here -- the next tick reconcile reinstalls the
    // boxes. These hooks fire after the built-in Selections component
    // has already cleared the list (plugins dispatch after core
    // components), so we never re-add ahead of the clear.
    fn reset(&mut self) {
        invalidate();
    }

    fn on_new_map_loaded(&mut self) {
        invalidate();
    }
}

/// Recompute and apply the desired selection set. Cheap no-op while the
/// want-state is unchanged.
fn reconcile() {
    // Only the AABBs scoped to the player's current map (see
    // `splits::visible_aabbs`), so a multi-map track shows just the
    // boxes that can fire where the player actually is.
    let want: Vec<(CheckpointKind, Aabb)> = if SHOW.get() {
        splits::visible_aabbs()
    } else {
        Vec::new()
    };

    if LAST_APPLIED.with_borrow(|last| *last == want) {
        return;
    }

    // Remove whatever the previous reconcile installed.
    ACTIVE_IDS.with_borrow_mut(|ids| {
        for id in ids.drain(..) {
            unsafe { Selections_Remove(id) };
        }
    });

    if want.len() > HUD_ID_COUNT {
        warn!(
            total = want.len(),
            cap = HUD_ID_COUNT,
            "more in-scope AABB checkpoints than the HUD id range; showing the first \
             {HUD_ID_COUNT}"
        );
    }

    ACTIVE_IDS.with_borrow_mut(|ids| {
        // zip stops at the shorter side, capping installs at HUD_ID_COUNT.
        for (id, (kind, aabb)) in (HUD_ID_BASE..=u8::MAX).zip(&want) {
            let p1 = ivec3(aabb.min);
            let p2 = ivec3(aabb.max);
            unsafe { Selections_Add(id, &p1, &p2, color_for_kind(*kind)) };
            ids.push(id);
        }
    });

    LAST_APPLIED.with_borrow_mut(|last| *last = want);
}

/// Drop the cached snapshot without removing selections (the caller
/// relies on the engine having already cleared its own list).
fn invalidate() {
    // Force the next reconcile to re-add: an empty cache never equals a
    // non-empty want, and if the want is also empty there's nothing to
    // draw anyway.
    LAST_APPLIED.with_borrow_mut(Vec::clear);
    ACTIVE_IDS.with_borrow_mut(Vec::clear);
}

/// Remove every selection in our private id range and forget them.
fn clear_all_selections() {
    for id in HUD_ID_BASE..=u8::MAX {
        unsafe { Selections_Remove(id) };
    }
    ACTIVE_IDS.with_borrow_mut(Vec::clear);
}

/// Convert a runtime float world coord to the block-grid `IVec3` the
/// selection API takes. The runtime AABB uses the half-open convention
/// `min <= p < max`, so the natural cast (e.g. min `(10,4,20)`, max
/// `(11,5,21)` for a 1x1x1 trigger) lands on the block-exclusive high
/// corner the engine wants to wrap the far block fully.
#[expect(
    clippy::cast_possible_truncation,
    reason = "world coords are non-negative and well within i32 after rounding"
)]
fn ivec3(v: Vec3) -> IVec3 {
    IVec3 {
        x: v.x.round() as i32,
        y: v.y.round() as i32,
        z: v.z.round() as i32,
    }
}

/// Alpha applied to every checkpoint box. The engine alpha-blends both
/// the translucent face fill and the (RGB-inverted) wireframe edges off
/// this same byte (`SelectionBox.c` BuildFaces / BuildEdges), so it's a
/// uniform fade -- kept low enough to read as "mostly transparent" but
/// high enough that the outline stays legible against the world.
const BOX_ALPHA: u8 = 64;

fn color_for_kind(kind: CheckpointKind) -> PackedCol {
    // `PackedCol_Make` is the classicube-sys const-fn wrapper around the
    // engine's `PackedCol_Make` macro -- it builds the word from the
    // platform-correct channel shifts, so we don't hand-roll the packing.
    match kind {
        CheckpointKind::Start => PackedCol_Make(0, 255, 0, BOX_ALPHA), // green
        CheckpointKind::Split => PackedCol_Make(255, 255, 0, BOX_ALPHA), // yellow
        CheckpointKind::Pause => PackedCol_Make(0, 200, 255, BOX_ALPHA), // cyan
        CheckpointKind::Resume => PackedCol_Make(255, 140, 0, BOX_ALPHA), // orange
        CheckpointKind::End => PackedCol_Make(255, 0, 0, BOX_ALPHA),   // red
    }
}
