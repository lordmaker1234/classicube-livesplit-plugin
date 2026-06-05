//! Route-line overlay connecting the checkpoint label anchors in track order.
//!
//! Each tick [`reconcile`] derives an ordered list of `(anchor, kind)`
//! waypoints from the same map-scoped `splits::visible_aabbs()` snapshot the
//! boxes and labels layers use. The render hook then draws a colored
//! camera-facing ribbon quad between each consecutive pair of waypoints so the
//! player can follow the route to the next checkpoint.
//!
//! **Rendering** shares the single `OwnedScreen` hook owned by `HudModule`
//! (in `hud/render.rs`). That hook invokes [`draw_pass`] first (so line
//! segments draw below labels), then the labels pass. Shared state the caller
//! sets up: 3D depth/blend/PROJ/VIEW, then 2D-ortho restore afterwards. This
//! pass only sets `VERTEX_FORMAT_COLOURED` and streams one quad per segment.
//! - 3D state: depth test **on** (lines are occluded by terrain, like the
//!   text), depth write **off**, alpha blending **on**.
//! - One `VERTEX_FORMAT_COLOURED` dynamic vertex buffer, sized 4 (streamed
//!   one quad at a time, like `draw_billboard`).
//!
//! **Ribbon math**: each segment A->B is a flat quad that turns to face the
//! camera. The width axis is `cross(B-A, eye-mid)` normalized, so the quad
//! lies in the plane containing the segment and the eye. Constant world-space
//! half-width [`LINE_HALF_WIDTH`] keeps the ribbon readable regardless of
//! distance. A degenerate segment (A==B or seen end-on) is skipped -- it
//! would produce a near-zero cross product and `Vec3_Normalize` has no
//! zero guard.
//!
//! **Segment color**: each segment is colored by its *destination* checkpoint's
//! kind, using the same [`shared::kind_rgb`] palette as the boxes
//! layer. The `is_next` flag is carried in the diff key for lockstep
//! invalidation but does not alter the line color (the next-target box/label
//! highlight is already prominent).

#[cfg(test)]
mod tests;

use std::{cell::RefCell, ptr};

use classicube_helpers::events::gfx::{ContextLostEventHandler, ContextRecreatedEventHandler};
use classicube_sys::{
    Camera, Gfx_DrawVb_IndexedTris, Gfx_LockDynamicVb, Gfx_SetVertexFormat, Gfx_UnlockDynamicVb,
    GfxResourceID, OwnedGfxVertexBuffer, PackedCol_Make, Vec3, VertexColoured,
    VertexFormat__VERTEX_FORMAT_COLOURED,
};
use tracing::debug;

use super::{HUD_ID_COUNT, shared};
use crate::plugin::{
    module::Module,
    splits::geometry::{Aabb, CheckpointKind},
};

/// World-space half-width of each ribbon segment in blocks (~0.12-block
/// total width). Tunable: larger values are more visible from afar; smaller
/// values are less cluttered near the boxes.
const LINE_HALF_WIDTH: f32 = 0.06;

/// Alpha for the route line. Opaque enough to read as a navigation guide,
/// distinct from the translucent box fills (`BOX_ALPHA = 64`).
const LINE_ALPHA: u8 = 200;

/// Vertices in one billboard quad (drawn as two tris via the engine's shared
/// index buffer, matching the label layer's `draw_billboard` pattern).
const QUAD_VERTICES: i32 = 4;

/// Guard threshold for the degenerate cross-product case. If the squared
/// length of the width vector falls at or below this, the segment is skipped
/// rather than normalizing a near-zero vector.
const EPS: f32 = 1e-6;

thread_local! {
    /// The shared dynamic vertex buffer for streaming one quad at a time.
    /// `None` while the GPU context is lost.
    static VB: RefCell<Option<OwnedGfxVertexBuffer>> = const { RefCell::new(None) };

    /// The live draw list: one `(anchor, kind)` entry per visible checkpoint,
    /// in track order. Rebuilt by [`reconcile`] when the visible set changes;
    /// read by the render hook each frame. The anchor is the label anchor
    /// (top-center of the AABB raised by [`shared::LABEL_Y_OFFSET`]).
    static WAYPOINTS: RefCell<Vec<(Vec3, CheckpointKind)>> = const { RefCell::new(Vec::new()) };

    /// Reconcile diff key: `(kind, aabb, is_next)` triples, the same shape
    /// [`super::boxes::BoxesModule`] uses so all three layers invalidate in
    /// lockstep. `is_next` is carried for lockstep only; it does not affect
    /// the line color.
    static LAST_SET: RefCell<Vec<(CheckpointKind, Aabb, bool)>> =
        const { RefCell::new(Vec::new()) };
}

fn vb_resource_id() -> Option<GfxResourceID> {
    VB.with_borrow(|vb| vb.as_ref().map(|b| b.resource_id))
}

fn subscribe() -> (ContextLostEventHandler, ContextRecreatedEventHandler) {
    let mut lost = ContextLostEventHandler::new();
    // On context loss only the vertex buffer is invalid; the waypoint cache is
    // plain CPU geometry (world coords + kinds, no GPU ids) and stays valid, so
    // we drop just the buffer and leave the cache for the next frame to draw
    // once the buffer is rebuilt. (The labels layer *must* also invalidate its
    // cache here, because that one holds `OwnedTexture` GPU ids -- this layer
    // doesn't.)
    lost.on(|_| drop_buffer());

    let mut recreated = ContextRecreatedEventHandler::new();
    recreated.on(|_| context_recreated());

    context_recreated();

    (lost, recreated)
}

fn drop_buffer() {
    VB.with_borrow_mut(|vb| drop(vb.take()));
}

fn context_recreated() {
    VB.with_borrow_mut(|vb| {
        *vb = OwnedGfxVertexBuffer::new(VertexFormat__VERTEX_FORMAT_COLOURED, QUAD_VERTICES);
    });
}

/// Clear the cached waypoint list and diff key so the next [`reconcile`]
/// rebuilds. For genuine cache-staleness only -- map change, reset, and
/// free. **Not** called on GPU context loss: the cache holds no GPU
/// resources, so only the vertex buffer needs dropping there.
fn invalidate() {
    WAYPOINTS.with_borrow_mut(Vec::clear);
    LAST_SET.with_borrow_mut(Vec::clear);
}

// ---- module struct ----

/// Owns the RAII registration handles. The vertex buffer, waypoints, and diff
/// key live in thread-locals (accessible from the context callbacks and
/// [`draw_pass`]); only the handles themselves -- which are never touched
/// from inside a callback -- are fields. The shared render hook is owned by
/// `HudModule` and is not a field here.
pub(super) struct LinesModule {
    _context_lost: ContextLostEventHandler,
    _context_recreated: ContextRecreatedEventHandler,
}

impl LinesModule {
    /// Subscribe to GPU context events (creating the VB now). The shared
    /// render hook is installed by `HudModule`.
    pub(super) fn init() -> Self {
        let (context_lost, context_recreated) = subscribe();
        Self {
            _context_lost: context_lost,
            _context_recreated: context_recreated,
        }
    }
}

impl Module for LinesModule {
    fn free(&mut self) {
        drop_buffer();
        invalidate();
        debug!("route lines freed");
    }

    fn reset(&mut self) {
        invalidate();
    }

    fn on_new_map_loaded(&mut self) {
        invalidate();
    }
}

// ---- reconcile ----

/// Derive the ordered waypoint list from the map-scoped visible checkpoint
/// set, for use by the render hook and tests.
pub(crate) fn derive_waypoints(
    visible: &[(usize, CheckpointKind, Aabb, String, bool)],
) -> Vec<(Vec3, CheckpointKind)> {
    visible
        .iter()
        .take(HUD_ID_COUNT)
        .map(|(_, k, a, ..)| (shared::label_anchor(a), *k))
        .collect()
}

/// Update the cached waypoint list to match the current map-scoped visible
/// set. No-op when the set is unchanged. Called once per tick by the HUD
/// coordinator alongside `boxes::reconcile` and `labels::reconcile`.
pub(super) fn reconcile(visible: &[(usize, CheckpointKind, Aabb, String, bool)]) {
    let want: Vec<(CheckpointKind, Aabb, bool)> = visible
        .iter()
        .take(HUD_ID_COUNT)
        .map(|(_, k, a, _, n)| (*k, *a, *n))
        .collect();

    if LAST_SET.with_borrow(|last| *last == want) {
        return;
    }

    let waypoints = derive_waypoints(visible);
    let count = waypoints.len();
    WAYPOINTS.with_borrow_mut(|wps| *wps = waypoints);
    LAST_SET.with_borrow_mut(|last| *last = want);
    debug!(count, "rebuilt route line waypoints");
}

// ---- render hook ----

/// Cross product of two `Vec3` vectors. The `classicube-sys` bindings do not
/// expose a cross-product helper, and `Vec3_Normalize` has no zero guard, so
/// we compute both inline to avoid any missing-method risk.
fn cross(u: Vec3, v: Vec3) -> Vec3 {
    Vec3::create(
        u.y * v.z - u.z * v.y,
        u.z * v.x - u.x * v.z,
        u.x * v.y - u.y * v.x,
    )
}

/// Stream a single camera-facing ribbon quad between two world-space anchor
/// points and draw it with the given destination-checkpoint kind color.
fn draw_segment(vb: GfxResourceID, eye: Vec3, pt_a: Vec3, pt_b: Vec3, dest: CheckpointKind) {
    // Build the width axis: perpendicular to both the segment direction and
    // the eye-to-midpoint direction, so the quad always faces the camera.
    let mid = (pt_a + pt_b) * 0.5;
    let width_vec = cross(pt_b - pt_a, eye - mid);

    // Degenerate case: pt_a == pt_b (zero segment) or segment points directly
    // at/away from the eye. The cross product is then ~0 and normalizing would
    // divide by sqrt(0) = NaN. An end-on line is invisible anyway; skip.
    let len_sq = width_vec.x * width_vec.x + width_vec.y * width_vec.y + width_vec.z * width_vec.z;
    if len_sq <= EPS {
        return;
    }

    let inv_len = 1.0_f32 / len_sq.sqrt();
    let w = Vec3::create(
        width_vec.x * inv_len * LINE_HALF_WIDTH,
        width_vec.y * inv_len * LINE_HALF_WIDTH,
        width_vec.z * inv_len * LINE_HALF_WIDTH,
    );

    let (col_r, col_g, col_b) = shared::kind_rgb(dest);
    let col = PackedCol_Make(col_r, col_g, col_b, LINE_ALPHA);

    let mk = |p: Vec3| VertexColoured {
        x: p.x,
        y: p.y,
        z: p.z,
        Col: col,
    };
    // Quad wound CCW matching the label layer's `Particle_DoRender` convention:
    // the engine's shared index buffer renders {0,1,2} and {2,3,0}.
    let verts = [mk(pt_a - w), mk(pt_a + w), mk(pt_b + w), mk(pt_b - w)];

    unsafe {
        let dst = Gfx_LockDynamicVb(vb, VertexFormat__VERTEX_FORMAT_COLOURED, 4);
        ptr::copy_nonoverlapping(verts.as_ptr(), dst.cast::<VertexColoured>(), 4);
        Gfx_UnlockDynamicVb(vb);
        Gfx_DrawVb_IndexedTris(4);
    }
}

/// Route-line draw pass: set the coloured vertex format, read the current eye
/// position, then stream one ribbon quad per consecutive waypoint pair.
/// Returns immediately if the VB is unavailable (GPU context lost or not yet
/// created). Called by the shared HUD render hook in `hud/render.rs` before
/// the labels pass so lines draw underneath the floating text.
///
/// The 3D state (depth/blend/proj/view) is set up by the caller before
/// invoking this pass, and the 2D-ortho state is restored by the caller
/// afterwards; this function only sets the vertex format and streams quads.
pub(super) fn draw_pass() {
    let Some(vb) = vb_resource_id() else {
        return;
    };
    // SAFETY: Camera.CurrentPos is a plain float read from a C global; safe to
    // read from the main thread during a render callback.
    let eye = unsafe { Camera.CurrentPos };
    unsafe {
        Gfx_SetVertexFormat(VertexFormat__VERTEX_FORMAT_COLOURED);
    }
    WAYPOINTS.with_borrow(|wps| {
        for w in wps.windows(2) {
            // Segment i -> i+1: color = destination (w[1]) kind.
            draw_segment(vb, eye, w[0].0, w[1].0, w[1].1);
        }
    });
}
