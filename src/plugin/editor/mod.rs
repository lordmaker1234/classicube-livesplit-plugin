//! In-world track editor: place / remove / relabel checkpoints with chat
//! commands and block clicks.
//!
//! `/client LiveSplit edit on` enables edit mode. While armed (via
//! `edit add`), the next two block clicks are consumed as the two
//! corners of a checkpoint AABB instead of building/breaking world blocks
//! -- the clicked block is reverted locally and never sent to the server.
//! The `Server.SendBlock` hook ([`hook`]) and build-permission override
//! ([`permissions`]) are installed only during an active armed capture and
//! restored immediately after the second click (or on cancel / map change).
//! The committed checkpoint flows through `splits::editor_add`, the
//! same `Track` mutation path the chat/`.lss` sources feed into, and
//! becomes visible through the existing `/client LiveSplit show` HUD on
//! the next tick.
//!
//! A **rubber-band preview** (selection 255, white translucent) shows the
//! tentative AABB while the player aims after clicking corner A; it updates
//! every frame via [`preview`] and disappears on commit or cancel.
//! Corner-A marker, selection highlight, and edit-mode status overlay are
//! still deferred to the track-editor HUD work.

pub mod hook;
mod permissions;
mod preview;

use std::{cell::RefCell, os::raw::c_int};

use classicube_helpers::tick::TickEventHandler;
use classicube_sys::{BlockID, Game_UpdateBlock, IVec3};
use tracing::debug;

use self::{hook::HookModule, permissions::PermissionsModule, preview::PreviewModule};
use crate::{
    chat_print,
    plugin::{
        module::Module,
        splits::{
            self,
            geometry::{self, Boundary, CheckpointKind, RetypeTarget},
        },
    },
};

/// Placeholder label committed when a checkpoint is placed via a block
/// click. Non-empty so the chat encoder / `.lss` writer accept it (both
/// require non-empty labels); override with
/// `/client LiveSplit edit label <i> <text>`.
const PLACEHOLDER_LABEL: &str = "checkpoint";

/// Which mutation a pending two-click capture commits to.
#[derive(Clone, Copy)]
enum PendingOp {
    /// `edit add [i]`: insert a checkpoint. `None` appends to the
    /// current map section; `Some(i)` inserts at index `i`.
    Add(Option<usize>),
    /// `edit redraw <i>`: replace the AABB of the existing checkpoint at
    /// `i`, keeping its kind / label / position.
    Redraw(usize),
}

/// A two-click capture armed via `edit add` / `edit redraw`, waiting
/// for its two corner clicks. `corner_a` is `None` until the first click
/// lands.
struct Pending {
    /// Which mutation the two corners commit to once both are clicked.
    op: PendingOp,
    corner_a: Option<IVec3>,
}

struct EditorState {
    /// Edit mode on/off (`edit on` / `edit off`). Must be on before a
    /// placement can be armed; turning it off disarms any half-armed one.
    enabled: bool,
    /// `Some` once `edit add` arms a placement; `None` otherwise.
    pending: Option<Pending>,
}

thread_local! {
    static EDITOR_STATE: RefCell<EditorState> = const {
        RefCell::new(EditorState {
            enabled: false,
            pending: None,
        })
    };
}

/// Whether edit mode is currently on (`edit on`). Read by the HUD label
/// layer to decide whether to annotate each label with its `(<kind>)`
/// suffix (an authoring aid, hidden during normal play).
pub fn is_enabled() -> bool {
    EDITOR_STATE.with_borrow(|s| s.enabled)
}

/// Reset the `EDITOR_STATE` thread-local to its initial values
/// (`enabled = false`, no pending placement). Shared by `free()` (teardown)
/// and `reset()` (disconnect / local-map-load clean slate). Does NOT touch
/// the `SendBlock` hook or permissions override (resources managed by
/// `hook::install`/`uninstall` and `permissions::apply`/`restore`).
fn reset_state() {
    EDITOR_STATE.with_borrow_mut(|s| {
        s.enabled = false;
        s.pending = None;
    });
}

/// Arm a two-click capture for `op`: the next two block clicks become a
/// checkpoint's corners. Installs the `SendBlock` hook and forces build
/// permissions for the duration of the capture (both torn down by
/// [`disarm`]). Requires edit mode on; prints a hint and returns `false`
/// otherwise. The single entry point that sets `pending` -- callers pair
/// it with their own "armed" chat line.
fn arm(op: PendingOp) -> bool {
    let armed = EDITOR_STATE.with_borrow_mut(|s| {
        if !s.enabled {
            return false;
        }
        s.pending = Some(Pending { op, corner_a: None });
        true
    });
    if armed {
        permissions::apply();
        hook::install();
    } else {
        chat_print("&cLiveSplit: enable edit mode first (/client LiveSplit edit on)");
    }
    armed
}

/// Drop any half-armed placement, uninstalling the `SendBlock` hook and
/// restoring block permissions if a capture was in progress. Returns
/// whether one was armed. The single exit point for `pending` -- every
/// path that abandons a placement (commit, `edit cancel`, `edit off`,
/// map change, `new` / `clear`) funnels through here, so the hook +
/// permission resources can never outlive the `pending` they're tied to.
/// Idempotent: a no-op (returns `false`) when nothing is armed.
fn disarm() -> bool {
    let was_armed = EDITOR_STATE.with_borrow_mut(|s| s.pending.take().is_some());
    if was_armed {
        hook::uninstall();
        permissions::restore();
    }
    was_armed
}

/// `edit on` / `edit off`. Turning off disarms any in-progress placement.
pub fn set_enabled(on: bool) {
    EDITOR_STATE.with_borrow_mut(|s| s.enabled = on);
    if on {
        chat_print("&aLiveSplit: edit mode ON");
        chat_print("&e  /client LiveSplit edit add, then click two blocks for a checkpoint");
        // Authoring isn't a timed attempt: abandon any in-progress run so
        // editing starts from a clean idle cursor. `with_timer_reset` brackets
        // the rearm so a connected timer mid-run gets reset too; it's a silent
        // no-op when nothing was running / no timer is attached.
        splits::with_timer_reset("to allow edit", splits::reset_run);
    } else {
        disarm();
        chat_print("&aLiveSplit: edit mode OFF");
    }
}

/// `edit add [i]`. Arm a placement: the next two block clicks become a
/// checkpoint's corners. `target` is `None` to append to the player's
/// current map section (before its terminating `MapLoaded`, or before
/// `End` on the last/only map) or `Some(i)` to insert at index `i`.
pub fn arm_add(target: Option<usize>) {
    if !arm(PendingOp::Add(target)) {
        return;
    }
    match target {
        None => chat_print("&aLiveSplit: armed - click corner A (appends to current map)"),
        Some(i) => chat_print(&format!(
            "&aLiveSplit: armed - click corner A (inserts at #{i})"
        )),
    }
}

/// `edit redraw <i>`. Arm a two-click capture that replaces the AABB of
/// the existing checkpoint at `i` (keeping its index, kind, and label)
/// instead of inserting a new one. No index pre-check here -- consistent
/// with [`arm_add`]; the authoritative range / `MapLoaded` check lives
/// in `splits::editor_relocate` and surfaces via chat at commit. The two
/// clicks revert locally, so a wasted arm leaves the map untouched.
pub fn arm_redraw(i: usize) {
    if !arm(PendingOp::Redraw(i)) {
        return;
    }
    chat_print(&format!(
        "&aLiveSplit: armed - click corner A (redraws checkpoint #{i})"
    ));
}

/// `edit cancel`. Discard a half-armed placement.
pub fn cancel() {
    if disarm() {
        chat_print("&aLiveSplit: placement cancelled");
    } else {
        chat_print("&eLiveSplit: nothing to cancel");
    }
}

/// `edit remove <i>`. Remove the checkpoint at `i`.
pub fn remove(i: usize) {
    splits::editor_remove(i);
}

/// `edit label <i> <text>`. Relabel a checkpoint.
pub fn set_label(i: usize, text: String) {
    splits::editor_set_label(i, text);
}

/// `edit rename <name>`. Rename the loaded track. Non-structural string
/// edit (no arming / block clicks), so -- like `remove` / `label` /
/// `move` / `kind` -- it doesn't gate on edit mode.
pub fn rename(name: String) {
    splits::editor_rename(name);
}

/// `edit move <from> <to>`. Reorder a checkpoint within the route: the
/// one at `from` lands at index `to`, shifting the rest. No arming /
/// block clicks (purely index-based), so -- like `remove` / `label` --
/// it doesn't gate on edit mode.
pub fn reindex(from: usize, to: usize) {
    splits::editor_reindex(from, to);
}

/// `edit kind <i> start|end|split|pause|resume`. For `split` / `pause` /
/// `resume`, retype the existing AABB checkpoint in place, keeping its
/// zone. For `start` / `end` -- position-derived boundary kinds, not
/// in-place retypes -- move the checkpoint to index 0 / the last index
/// (demoting the displaced former boundary to `Split`) via
/// [`splits::editor_set_boundary`]. No arming / block clicks (purely
/// index-based), so -- like `remove` / `label` / `move` -- it doesn't
/// gate on edit mode.
pub fn set_kind(i: usize, kind: CheckpointKind) {
    match kind {
        CheckpointKind::Start => splits::editor_set_boundary(i, Boundary::Start),
        CheckpointKind::End => splits::editor_set_boundary(i, Boundary::End),
        _ => splits::editor_set_kind(i, RetypeTarget::Aabb(kind)),
    };
}

/// `edit kind <i> map [name]`. Convert checkpoint #i into a zoneless map
/// transition. `name` defaults to the live world (`splits::current_map`);
/// errors in chat if a name is neither given nor resolvable.
pub fn set_kind_map(i: usize, name: Option<String>) {
    match name.or_else(splits::current_map) {
        Some(n) => {
            splits::editor_set_kind(i, RetypeTarget::Map(n));
        }
        None => chat_print("&cLiveSplit: no map name given and current map is unknown"),
    }
}

/// `edit new <name>`. Start authoring a brand-new empty track named
/// `name`, scoped to the current map. Auto-enables edit mode (prints
/// "edit mode ON", resets any in-progress run) so the next clicks place
/// checkpoints immediately. Replaces any currently loaded track.
pub fn new_track(name: String) {
    if !is_enabled() {
        set_enabled(true);
    }
    disarm(); // drop any half-armed placement
    splits::new_track(name);
}

/// `edit clear`. Drop the loaded track and reset editor state so the
/// player can author a fresh one from scratch.
pub fn clear() {
    splits::clear_track();
    disarm(); // drop any half-armed placement
}

/// What a `SendBlock` click resolved to after consulting editor state.
enum ClickOutcome {
    /// Not in edit mode / not armed: let the block change flow through.
    Ignore,
    /// First corner recorded; revert the block and wait for corner B.
    CornerA,
    /// Second corner recorded; revert the block and commit the AABB.
    CornerB { a: IVec3, b: IVec3, op: PendingOp },
}

/// Called from the [`hook`]'s `Server.SendBlock` override for every block
/// the player places/breaks. Returns `true` when the editor consumed the
/// click as a checkpoint corner (the caller then suppresses the
/// server-notify), `false` to pass the block change through unchanged.
///
/// When a placement is armed, the click is always consumed: the local
/// world block the engine already mutated is reverted via
/// `Game_UpdateBlock` (which does NOT re-enter `SendBlock`, so no
/// recursion), then recorded as corner A or combined with A into a
/// committed checkpoint.
pub(super) fn consume_click(x: c_int, y: c_int, z: c_int, old: BlockID) -> bool {
    // Scope the EDITOR_STATE borrow to the state-machine decision; the
    // revert + splits mutation + `disarm()` happen after it's released so
    // the SendBlock hook can't double-borrow editor state, and
    // `editor_add`'s own splits borrow stays independent. Corner B leaves
    // `pending` set here and lets the post-borrow `disarm()` clear it --
    // the single exit point that also tears down the hook + permissions.
    let outcome = EDITOR_STATE.with_borrow_mut(|s| {
        if !s.enabled {
            return ClickOutcome::Ignore;
        }
        let Some(pending) = s.pending.as_mut() else {
            return ClickOutcome::Ignore;
        };
        match pending.corner_a {
            None => {
                pending.corner_a = Some(IVec3 { x, y, z });
                ClickOutcome::CornerA
            }
            Some(a) => ClickOutcome::CornerB {
                a,
                b: IVec3 { x, y, z },
                op: pending.op,
            },
        }
    });

    match outcome {
        ClickOutcome::Ignore => false,
        ClickOutcome::CornerA => {
            revert_block(x, y, z, old);
            chat_print("&aLiveSplit: corner A set; click corner B");
            true
        }
        ClickOutcome::CornerB { a, b, op } => {
            revert_block(x, y, z, old);
            let aabb = geometry::aabb_from_block_corners(a, b);
            match op {
                PendingOp::Add(target) => {
                    splits::editor_add(aabb, PLACEHOLDER_LABEL.to_owned(), target);
                }
                PendingOp::Redraw(i) => {
                    splits::editor_relocate(i, aabb);
                }
            }
            disarm();
            true
        }
    }
}

/// Restore the block the engine already changed locally. `Game_UpdateBlock`
/// updates only the local world (unlike `Game_ChangeBlock` it does not
/// call back into `Server.SendBlock`), so the edit click leaves the map
/// untouched and there's no hook recursion.
fn revert_block(x: c_int, y: c_int, z: c_int, old: BlockID) {
    // SAFETY: `Game_UpdateBlock` is a `CC_API` engine fn; we're on the
    // main thread (input dispatch), with in-range world coords supplied
    // by the engine's own `SendBlock` call.
    unsafe {
        Game_UpdateBlock(x, y, z, old);
    }
}

pub struct EditorModule {
    // Resource-lifecycle children. `children()` returns them in
    // `[hook, permissions, preview]` order so reverse-dispatch frees
    // preview then permissions then hook.
    hook: HookModule,
    permissions: PermissionsModule,
    preview: PreviewModule,
    // Owned for its Drop side-effect: TickEventHandler::Drop unregisters
    // the closure. Drives the per-frame `preview::reconcile()` (no
    // per-frame `Module` hook exists), mirroring `HudModule`'s tick driving
    // its children's `reconcile`.
    _tick: TickEventHandler,
}

impl EditorModule {
    pub fn init() -> Self {
        let hook = HookModule::init();
        let permissions = PermissionsModule::init();
        let preview = PreviewModule::init();

        let mut tick = TickEventHandler::new();
        tick.on(move |_| {
            preview::reconcile();
        });
        Self {
            hook,
            permissions,
            preview,
            _tick: tick,
        }
    }
}

impl Module for EditorModule {
    fn children(&mut self) -> Vec<&mut dyn Module> {
        vec![&mut self.hook, &mut self.permissions, &mut self.preview]
    }

    fn free(&mut self) {
        // `hook` / `preview` children already cleared their own resources
        // (reverse-dispatch runs them before this); clear the editor's own
        // thread-local state.
        reset_state();
        debug!("EditorModule freed; editor state cleared");
        // `_tick` unregisters via its own Drop after `free` returns; no
        // render or tick event fires during synchronous teardown.
    }

    fn reset(&mut self) {
        // End any authoring session on disconnect / local-map-load.
        // `set_enabled(false)` does the meaningful work (uninstalls the
        // hook, prints "OFF") only when edit mode was on; `reset_state()`
        // then guarantees a clean slate regardless. The `preview` child's
        // own `reset()` clears the rubber-band selection.
        if is_enabled() {
            set_enabled(false);
        }
        reset_state();
    }

    fn on_new_map_loaded(&mut self) {
        // A half-armed placement holds corner coords from the old map;
        // they're meaningless after a map change. Drop it and tear down
        // the hook + permission override. The `preview` child invalidates
        // its cache via its own `on_new_map_loaded`.
        disarm();
    }
}
