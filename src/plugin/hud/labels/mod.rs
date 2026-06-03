//! Floating text labels above the in-world checkpoint boxes.
//!
//! The boxes are engine selection cuboids (see the parent module) and can't
//! carry text -- the CPE label slot is parsed off the packet and discarded.
//! This layer adds real GPU text: per checkpoint, a drop-shadowed texture of
//! its `<label> (<kind>)` annotation tinted to the box's kind hue, drawn each
//! frame as a camera-facing billboard anchored just above its box (occluded
//! by terrain, shrinking with distance).
//!
//! Split across three submodules:
//! - [`texture`] -- label string to `OwnedTexture` (lazy font, transparent
//!   canvas, `Gfx.LostContext` guard);
//! - [`context`] -- the shared dynamic vertex buffer + context lost/recreated
//!   handlers;
//! - [`render`] -- the `OwnedScreen` render hook and billboard math.
//!
//! The cache is driven by the same tick + `SHOW` + map-scoped
//! `splits::visible_aabbs()` data the boxes use: [`reconcile`] rebuilds the
//! texture set only when the visible `(kind, aabb, label, is_next)` set
//! changes, so a map crossing (which changes the scoped set) invalidates the
//! cache for free and label-only edits to off-screen checkpoints don't churn
//! it. Each label is tinted to its `kind` hue (matching the box color).

mod context;
mod render;
mod texture;

#[cfg(test)]
mod tests;

use std::cell::RefCell;

use classicube_helpers::events::gfx::{ContextLostEventHandler, ContextRecreatedEventHandler};
use classicube_sys::{Gfx, OwnedScreen, OwnedTexture, Vec3};
use tracing::debug;

use super::HUD_ID_COUNT;
use crate::plugin::{
    module::Module,
    splits::geometry::{Aabb, CheckpointKind, kind_name},
};

/// Vertical gap (in blocks) between the top of a box and the bottom of its
/// label, so the text floats just clear of the cuboid's top face.
const LABEL_Y_OFFSET: f32 = 0.3;

/// Target world height (in blocks) of a single line of label text. The
/// pixel->world scale is derived per-label from this and the rendered text
/// height, so every label is the same on-screen height regardless of the
/// font's pixel metrics; width scales proportionally. Tunable.
const LABEL_LINE_WORLD_HEIGHT: f32 = 0.4;

/// One cached, ready-to-draw label.
struct Label {
    /// Bottom-center world anchor: the top-center of the box raised by
    /// [`LABEL_Y_OFFSET`]. The billboard renderer raises this by half the
    /// quad height internally, so the text sits just above the box.
    anchor: Vec3,
    /// `(width, height)` of the billboard quad in world units (blocks).
    size_world: (f32, f32),
    tex: OwnedTexture,
}

thread_local! {
    /// The cached label set, drawn by the render hook each frame. Rebuilt by
    /// [`reconcile`] when the visible set changes.
    static LABELS: RefCell<Vec<Label>> = const { RefCell::new(Vec::new()) };

    /// The `(kind, aabb, label, is_next)` set [`LABELS`] was last built
    /// from -- the reconcile diff key (the raw `visible` 4-tuple, not the
    /// decorated display string). Mirrors the boxes' `LAST_APPLIED` cache so
    /// the two layers invalidate in lockstep on a map crossing, and carrying
    /// `is_next` rebuilds the textures when the run cursor advances (the
    /// marker/color moves to the new next checkpoint).
    static LAST_LABEL_SET: RefCell<Vec<(CheckpointKind, Aabb, String, bool)>> =
        const { RefCell::new(Vec::new()) };
}

/// Owns the layer's RAII registration handles. Everything else this layer
/// needs (the vertex buffer, label textures, font) is reached from the
/// `extern "C"` render hook or the `'static` context closures, so it lives
/// in thread-locals these handles' callbacks poke; only the handles
/// themselves -- which are never touched from inside a callback -- can be
/// fields. Dropping the module unregisters all three (like
/// `HudModule._tick`).
pub(super) struct LabelsModule {
    // The HUD render-hook screen; Drop calls Gui_Remove.
    _screen: OwnedScreen,
    // GPU context lost/recreated subscriptions; Drop unregisters them. Their
    // closures rebuild/drop the thread-local vertex buffer in `context`.
    _context_lost: ContextLostEventHandler,
    _context_recreated: ContextRecreatedEventHandler,
}

impl LabelsModule {
    /// Subscribe to the context events (creating the vertex buffer now) and
    /// register the render hook. The font is built lazily on first label.
    pub(super) fn init() -> Self {
        let (context_lost, context_recreated) = context::subscribe();
        let screen = render::install();
        Self {
            _screen: screen,
            _context_lost: context_lost,
            _context_recreated: context_recreated,
        }
    }
}

impl Module for LabelsModule {
    /// Release the thread-local GPU resources the callbacks force out of this
    /// struct: the vertex buffer, the cached label textures, and the font.
    /// The `_screen` / `_context_*` fields unregister via their own Drop
    /// right after this returns -- no render or context event fires during
    /// synchronous teardown, and the render hook no-ops on a dropped buffer.
    fn free(&mut self) {
        context::drop_buffer();
        invalidate();
        texture::free();
        debug!("floating checkpoint labels freed");
    }

    // Stale-scope labels shouldn't linger a frame after a map change /
    // reset; drop the cache so the next tick rebuilds from the new map's
    // scoped set. (Mirrors the box layer's invalidate-on-reset.)
    fn reset(&mut self) {
        invalidate();
    }

    fn on_new_map_loaded(&mut self) {
        invalidate();
    }
}

/// Rebuild the cached label textures to match `visible` (the map-scoped
/// `(kind, aabb, label, is_next)` set, already capped/derived by the splits
/// layer). No-op when `visible` equals the cached set. The `kind` drives the
/// label color (matching the box hue) and the ` (<kind>)` suffix via
/// [`display_label`]; `is_next` adds the `> ` marker on the run's next-target
/// label. Both are part of the diff key so the cache tracks the boxes
/// exactly.
pub(super) fn reconcile(visible: &[(CheckpointKind, Aabb, String, bool)]) {
    // While the GPU context is lost we can't (re)build textures. Leave the
    // cache exactly as `context::context_lost`'s `invalidate()` left it
    // (empty) and bail, so a tick landing in the lost window doesn't
    // repopulate `LAST_LABEL_SET` and defeat the rebuild-after-recreate.
    if unsafe { Gfx.LostContext } != 0 {
        return;
    }

    if LAST_LABEL_SET.with_borrow(|last| last.as_slice() == visible) {
        return;
    }

    // Cap at the box id range so labels stay in lockstep with the boxes that
    // actually draw (the parent reconcile zips installs against the same
    // range).
    let mut labels = Vec::new();
    for (kind, aabb, label, is_next) in visible.iter().take(HUD_ID_COUNT) {
        let display = display_label(*kind, label, *is_next);
        let Some(tex) = texture::create_label_texture(&display) else {
            continue;
        };
        let (px_w, px_h) = {
            let t = tex.as_texture();
            (f32::from(t.width), f32::from(t.height))
        };
        let scale = LABEL_LINE_WORLD_HEIGHT / px_h;
        let size_world = (px_w * scale, px_h * scale);
        let anchor = Vec3::create(
            (aabb.min.x + aabb.max.x) / 2.0,
            aabb.max.y + LABEL_Y_OFFSET,
            (aabb.min.z + aabb.max.z) / 2.0,
        );
        labels.push(Label {
            anchor,
            size_world,
            tex,
        });
    }

    let count = labels.len();
    LABELS.with_borrow_mut(|slot| *slot = labels);
    LAST_LABEL_SET.with_borrow_mut(|last| {
        last.clear();
        last.extend_from_slice(visible);
    });
    debug!(count, "rebuilt floating checkpoint labels");
}

/// Build the floating display string for a checkpoint: the raw `label` (when
/// set) plus a ` (<kind>)` suffix, tinted to the box's kind hue. The run's
/// next-target gets a leading `&e> ` marker (yellow positional cue that
/// composes with the kind color following it). Unlabeled checkpoints still
/// render `(<kind>)` so every box is identifiable. The kind color is
/// re-asserted before the suffix so a label carrying its own `&` codes can't
/// bleed into the annotation.
fn display_label(kind: CheckpointKind, label: &str, is_next: bool) -> String {
    let code = kind_color_code(kind);
    let name = kind_name(kind).to_ascii_lowercase();
    let body = if label.is_empty() {
        format!("{code}({name})")
    } else {
        format!("{code}{label} {code}({name})")
    };
    if is_next { format!("&e> {body}") } else { body }
}

/// The `&`-code whose hue matches `boxes::color_for_kind`'s `PackedCol` for
/// this kind (hud/boxes.rs). The two hue tables are deliberately separate
/// (PackedCol vs `&`-code, different types); keep them in sync if a hue ever
/// changes.
fn kind_color_code(kind: CheckpointKind) -> &'static str {
    match kind {
        CheckpointKind::Start => "&a",  // green  (0,255,0)
        CheckpointKind::Split => "&e",  // yellow (255,255,0)
        CheckpointKind::Pause => "&b",  // cyan   (0,200,255)
        CheckpointKind::Resume => "&6", // orange (255,140,0)
        CheckpointKind::End => "&c",    // red    (255,0,0)
    }
}

/// Drop the cached textures and the diff key, forcing the next [`reconcile`]
/// to rebuild. Used by [`LabelsModule`]'s map-change / reset hooks
/// (stale-scope labels shouldn't linger) and on GPU context loss (the
/// texture ids are now dangling -- see [`context`]).
fn invalidate() {
    LABELS.with_borrow_mut(Vec::clear);
    LAST_LABEL_SET.with_borrow_mut(Vec::clear);
}
