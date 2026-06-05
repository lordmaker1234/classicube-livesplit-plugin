//! GPU vertex-buffer lifecycle for the label billboards, tied to the
//! engine's graphics context.
//!
//! Ported from
//! `classicube-chat-bubbles-plugin/src/plugin/rendering/context/{mod,vertex_buffer}.rs`,
//! trimmed to just the single shared dynamic vertex buffer the billboard
//! renderer streams into each frame (the chat-bubbles `Texture_Render` /
//! `Gfx_Make2DQuad` 2D-quad path isn't needed -- we build world-space verts
//! directly via `Particle_DoRender`).
//!
//! The buffer itself stays in a thread-local: it's read by the `extern "C"`
//! render hook (via [`vb_resource_id`]) and rebuilt/dropped by the context
//! event closures, none of which can reach a `&mut self`. The *subscription
//! handles* those closures live in, however, are owned by `LabelsModule` as
//! fields (see [`subscribe`]) -- dropping the module unregisters them.
//!
//! On context loss the buffer is dropped and the cached label textures are
//! invalidated (they're now dangling GPU ids); on recreation the buffer is
//! rebuilt and the empty cache repopulates on the next tick.

use std::cell::RefCell;

use classicube_helpers::events::gfx::{ContextLostEventHandler, ContextRecreatedEventHandler};
use classicube_sys::{GfxResourceID, OwnedGfxVertexBuffer, VertexFormat__VERTEX_FORMAT_TEXTURED};

/// Vertices in a single billboard quad (a textured quad drawn as two
/// triangles via the engine's shared index buffer).
const QUAD_VERTICES: i32 = 4;

thread_local! {
    static TEX_VB: RefCell<Option<OwnedGfxVertexBuffer>> = const { RefCell::new(None) };
}

/// The dynamic vertex buffer's resource id, or `None` while the context is
/// lost. The render hook skips drawing when this is `None`.
pub(super) fn vb_resource_id() -> Option<GfxResourceID> {
    TEX_VB.with_borrow(|vb| vb.as_ref().map(|b| b.resource_id))
}

/// Subscribe to the context lost/recreated events and create the buffer now
/// (the context already exists when the plugin loads, so we don't wait for a
/// recreate event that may never come). The returned handles are owned by
/// `LabelsModule`; dropping them unregisters the listeners.
pub(super) fn subscribe() -> (ContextLostEventHandler, ContextRecreatedEventHandler) {
    let mut lost = ContextLostEventHandler::new();
    lost.on(|_| context_lost());

    let mut recreated = ContextRecreatedEventHandler::new();
    recreated.on(|_| context_recreated());

    context_recreated();

    (lost, recreated)
}

/// Drop the buffer on plugin teardown. Unlike [`context_lost`] this leaves
/// the texture cache alone -- `LabelsModule::free` invalidates it explicitly.
pub(super) fn drop_buffer() {
    TEX_VB.with_borrow_mut(|vb| drop(vb.take()));
}

fn context_recreated() {
    TEX_VB.with_borrow_mut(|vb| {
        *vb = OwnedGfxVertexBuffer::new(VertexFormat__VERTEX_FORMAT_TEXTURED, QUAD_VERTICES);
    });
}

fn context_lost() {
    drop_buffer();
    // Cached textures hold GPU ids that are now invalid; drop them so the
    // next post-recreation tick rebuilds from scratch.
    super::invalidate();
}
