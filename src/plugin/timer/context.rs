//! GPU vertex-buffer lifecycle for the timer overlay's 2D quads.
//!
//! Ported from `hud/labels/context.rs` with the same Drop-on-lost /
//! rebuild-on-recreated pattern.

use std::cell::RefCell;

use classicube_helpers::events::gfx::{ContextLostEventHandler, ContextRecreatedEventHandler};
use classicube_sys::{GfxResourceID, OwnedGfxVertexBuffer, VertexFormat__VERTEX_FORMAT_TEXTURED};

const QUAD_VERTICES: i32 = 4;

thread_local! {
    static TEX_VB: RefCell<Option<OwnedGfxVertexBuffer>> = const { RefCell::new(None) };
}

pub fn vb_resource_id() -> Option<GfxResourceID> {
    TEX_VB.with_borrow(|vb| vb.as_ref().map(|b| b.resource_id))
}

pub fn subscribe() -> (ContextLostEventHandler, ContextRecreatedEventHandler) {
    let mut lost = ContextLostEventHandler::new();
    lost.on(|_| context_lost());

    let mut recreated = ContextRecreatedEventHandler::new();
    recreated.on(|_| context_recreated());

    context_recreated();
    (lost, recreated)
}

pub fn drop_buffer() {
    TEX_VB.with_borrow_mut(|vb| drop(vb.take()));
}

fn context_recreated() {
    TEX_VB.with_borrow_mut(|vb| {
        *vb = OwnedGfxVertexBuffer::new(VertexFormat__VERTEX_FORMAT_TEXTURED, QUAD_VERTICES);
    });
}

fn context_lost() {
    drop_buffer();
    super::invalidate_cache();
}
