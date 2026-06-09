//! Block build-permission override for the in-world editor.
//!
//! In ClassiCube, block placing and deleting are gated by the per-block arrays
//! `Blocks.CanPlace[]` and `Blocks.CanDelete[]`. A server that disables
//! building zeroes these, so `InputHandler_DeleteBlock`/`PlaceBlock` bail
//! before `Game_ChangeBlock` -- and therefore before `Server.SendBlock` --
//! making the editor's block-click hook invisible to those clicks.
//!
//! The fix: while a corner capture is armed (`edit add` / `edit redraw`),
//! force both arrays to all-`1` so the engine reaches `Server.SendBlock` even
//! in build-locked maps. The override is active only for the duration of the
//! two-click capture and is removed as soon as the second click commits the
//! checkpoint (or on cancel / map change / `edit off`).
//!
//! The real permission values are saved once when the capture is armed and
//! restored when it ends. If the server re-sends permissions while the
//! override is active (via the CPE `SetBlockPermission` packet), the
//! `BlockEvents.PermissionsChanged` handler updates the snapshot with the new
//! server values and re-forces the arrays, so `restore()` always gives back
//! the server's current intent.

use std::cell::RefCell;

use classicube_helpers::events::block::PermissionsChangedEventHandler;
use classicube_sys::Blocks;

use crate::plugin::module::Module;

struct SavedPerms {
    can_place: Vec<u8>,
    can_delete: Vec<u8>,
}

thread_local! {
    // `Some` == override is active (edit mode on); `None` == inactive.
    static SAVED: RefCell<Option<SavedPerms>> = const { RefCell::new(None) };
}

/// Read the live permission arrays and build a snapshot, then force every
/// entry to `1`. No-op if the override is already active (idempotent).
///
/// Called from `arm_add` / `arm_redraw` when a capture is armed.
pub(super) fn apply() {
    SAVED.with_borrow_mut(|saved| {
        if saved.is_some() {
            return; // already overriding; don't re-snapshot
        }
        let ptr = &raw mut Blocks;
        // SAFETY: `Blocks` is a CC_VAR global; main-thread access only.
        let (can_place, can_delete) = unsafe {
            let cp = (*ptr).CanPlace.to_vec();
            let cd = (*ptr).CanDelete.to_vec();
            (cp, cd)
        };
        *saved = Some(SavedPerms {
            can_place,
            can_delete,
        });
        force_all(ptr);
    });
}

/// Restore the snapshotted permission arrays and clear the saved state.
///
/// Called from `disarm()` when a capture ends (commit, cancel, or map change).
pub(super) fn restore() {
    SAVED.with_borrow_mut(|saved| {
        if let Some(s) = saved.take() {
            let ptr = &raw mut Blocks;
            // SAFETY: as above.
            unsafe {
                (*ptr).CanPlace.copy_from_slice(&s.can_place);
                (*ptr).CanDelete.copy_from_slice(&s.can_delete);
            }
        }
    });
}

/// Called when `BlockEvents.PermissionsChanged` fires. The engine has
/// already written the new server values into the arrays; snapshot them as
/// the new baseline (so `restore()` gives back the server's current intent,
/// not the stale values from when edit mode was first enabled), then
/// re-force the arrays to all-`1`.
pub(super) fn reassert() {
    SAVED.with_borrow_mut(|saved| {
        if let Some(s) = saved.as_mut() {
            let ptr = &raw mut Blocks;
            // SAFETY: `Blocks` is a CC_VAR global; main-thread access only.
            unsafe {
                s.can_place.copy_from_slice(&(*ptr).CanPlace);
                s.can_delete.copy_from_slice(&(*ptr).CanDelete);
            }
            force_all(ptr);
        }
    });
}

fn force_all(ptr: *mut classicube_sys::_BlockLists) {
    // SAFETY: `Blocks` is a CC_VAR global; main-thread access only.
    unsafe {
        (*ptr).CanPlace.fill(1);
        (*ptr).CanDelete.fill(1);
    }
}

/// Owns the `BlockEvents.PermissionsChanged` subscription and acts as the
/// `EditorModule` child for the permission-override lifecycle.
///
/// `free`/`reset` restore the permission arrays as a redundant safety net --
/// the same pattern `HookModule` uses: `set_enabled(false)` in the parent
/// already calls `restore()`, but having the child own its own teardown means
/// the arrays are restored even if the parent logic changes.
pub(super) struct PermissionsModule {
    _perms_changed: PermissionsChangedEventHandler,
}

impl PermissionsModule {
    pub(super) fn init() -> Self {
        let mut perms_changed = PermissionsChangedEventHandler::new();
        perms_changed.on(|_| {
            reassert();
        });
        Self {
            _perms_changed: perms_changed,
        }
    }
}

impl Module for PermissionsModule {
    fn free(&mut self) {
        restore();
    }

    fn reset(&mut self) {
        restore();
    }
}
