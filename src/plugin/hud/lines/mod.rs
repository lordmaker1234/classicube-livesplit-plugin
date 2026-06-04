//! Route-line overlay connecting the checkpoint label anchors in track order.
//!
//! Each tick [`reconcile`] derives an ordered list of `(anchor, kind)`
//! waypoints from the same map-scoped `splits::visible_aabbs()` snapshot the
//! boxes and labels layers use. The render hook then draws a colored
//! camera-facing ribbon quad between each consecutive pair of waypoints so the
//! player can follow the route to the next checkpoint.
//!
//! **Rendering** mirrors the label-billboard layer:
//! - An `OwnedScreen` hook running at `HUD - 3` (below labels at `HUD - 2`
//!   and chat-bubbles at `HUD - 1`, so the floating text stays legible on top
//!   of each line segment).
//! - 3D state: depth test **on** (lines are occluded by terrain, like the
//!   text), depth write **off**, alpha blending **on**.
//! - The PROJ/VIEW matrices are loaded from `Gfx.Projection` / `Gfx.View`,
//!   then restored to 2D ortho after drawing.
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

use std::{cell::RefCell, ffi::c_void, ptr};

use classicube_helpers::events::gfx::{ContextLostEventHandler, ContextRecreatedEventHandler};
use classicube_sys::{
    Camera, Game, Gfx, Gfx_DrawVb_IndexedTris, Gfx_LoadMatrix, Gfx_LockDynamicVb,
    Gfx_SetAlphaBlending, Gfx_SetAlphaTest, Gfx_SetDepthTest, Gfx_SetDepthWrite,
    Gfx_SetVertexFormat, Gfx_UnlockDynamicVb, GfxResourceID, Matrix, MatrixType__MATRIX_PROJ,
    MatrixType__MATRIX_VIEW, OwnedGfxVertexBuffer, OwnedScreen, PackedCol_Make, Vec3,
    VertexColoured, VertexFormat__VERTEX_FORMAT_COLOURED, screen::Priority,
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
/// (defensively) init / free. **Not** called on GPU context loss: the cache
/// holds no GPU resources, so only the vertex buffer needs dropping there.
fn invalidate() {
    WAYPOINTS.with_borrow_mut(Vec::clear);
    LAST_SET.with_borrow_mut(Vec::clear);
}

// ---- module struct ----

/// Owns the RAII registration handles. The vertex buffer, waypoints, and diff
/// key live in thread-locals (accessible from the `extern "C"` render hook
/// and context callbacks); only the handles themselves -- which are never
/// touched from inside a callback -- are fields.
pub(super) struct LinesModule {
    _screen: OwnedScreen,
    _context_lost: ContextLostEventHandler,
    _context_recreated: ContextRecreatedEventHandler,
}

impl LinesModule {
    /// Subscribe to GPU context events (creating the VB now) and register the
    /// render hook.
    pub(super) fn init() -> Self {
        // Defensive reset: these thread-locals persist across
        // Init -> Free -> Init in the same process (ClassiCube never
        // `dlclose`s the .so), and an abnormal teardown can skip `free`. Clear
        // any stale draw state so a fresh Init always starts clean -- mirrors
        // the boxes layer's init-time selection sweep. (`subscribe` rebuilds
        // the vertex buffer below, dropping any stale one.)
        invalidate();

        let (context_lost, context_recreated) = subscribe();
        let screen = install();
        Self {
            _screen: screen,
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

/// Called from `Gui_RenderGui` between `Gfx_Begin2D` and the HUD screen's
/// render. Switch to 3D state, draw the route ribbons, then restore the 2D
/// state the HUD (and later screens) expect.
unsafe extern "C" fn render(_elem: *mut c_void, _delta: f32) {
    unsafe {
        if Gfx.LostContext != 0 {
            return;
        }
        let Some(vb) = vb_resource_id() else {
            return;
        };

        // Read the eye position once before the draw loop.
        let eye = Camera.CurrentPos;

        // 3D state: depth test on (lines are occluded by terrain, like the
        // floating labels), depth write off, alpha blending on.
        Gfx_SetDepthTest(1);
        Gfx_SetDepthWrite(0);
        Gfx_SetAlphaBlending(1);
        Gfx_LoadMatrix(MatrixType__MATRIX_PROJ, &raw const Gfx.Projection);
        Gfx_LoadMatrix(MatrixType__MATRIX_VIEW, &raw const Gfx.View);
        Gfx_SetVertexFormat(VertexFormat__VERTEX_FORMAT_COLOURED);

        WAYPOINTS.with_borrow(|wps| {
            for w in wps.windows(2) {
                // Segment i -> i+1: color = destination (w[1]) kind.
                draw_segment(vb, eye, w[0].0, w[1].0, w[1].1);
            }
        });

        // Reconstruct the 2D ortho + identity view + HUD state `Gfx_Begin2D`
        // had loaded so the chatbox / hotbar / crosshair draw correctly after
        // us. Must use a backend-correct ortho formula (see
        // `shared::calc_ortho_matrix`).
        #[expect(
            clippy::cast_precision_loss,
            reason = "window dimensions are small positive ints"
        )]
        let ortho =
            shared::calc_ortho_matrix(Game.Width as f32, Game.Height as f32, -100.0, 1000.0);
        Gfx_LoadMatrix(MatrixType__MATRIX_PROJ, &ortho);
        Gfx_LoadMatrix(MatrixType__MATRIX_VIEW, &Matrix::IDENTITY);
        Gfx_SetAlphaBlending(1);
        Gfx_SetDepthWrite(0);
        Gfx_SetDepthTest(0);
        // ClassiCube's convention (matching `EntityNames_Render`) leaves
        // alpha-test off; otherwise translucent HUD gradients get their
        // <128-alpha pixels discarded.
        Gfx_SetAlphaTest(0);
    }
}

/// Build and register the route-line render hook at `HUD - 3` (one slot
/// below the label billboards at `HUD - 2`), so the floating text draws on
/// top of the line passing through it.
fn install() -> OwnedScreen {
    let mut screen = OwnedScreen::new();
    screen.on_render(render);
    // HUD - 3 (= 7): below labels (HUD - 2 = 8) and chat-bubbles (HUD - 1 =
    // 9), but above the 3D world pass, so lines are occluded by terrain but
    // drawn under the text overlays.
    screen.add(Priority::Custom(Priority::Hud.to_u8() - 3));
    screen
}
