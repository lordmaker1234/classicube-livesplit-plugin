//! Camera-facing billboard draw pass for the floating checkpoint labels.
//!
//! Exposes [`draw_pass`], invoked by the shared HUD render hook
//! (`hud/render.rs`) after the lines pass so labels draw on top. Sets
//! `VERTEX_FORMAT_TEXTURED` and streams each cached label as a world-anchored
//! billboard above its box. The shared 3D-state setup and 2D-ortho restore are
//! owned by the caller.
//!
//! The billboard math is the engine's name-tag technique: world-space verts
//! built from the camera's right/up basis (the `Gfx.View` rows), via the
//! `Particle_DoRender` helper -- the same path `EntityRenderers.c:DrawName`
//! and `Particle.c:Particle_DoRender` use.

use std::ptr;

use classicube_sys::{
    Gfx_BindTexture, Gfx_DrawVb_IndexedTris, Gfx_LockDynamicVb, Gfx_SetVertexFormat,
    Gfx_UnlockDynamicVb, GfxResourceID, PackedCol_Make, Particle_DoRender, Vec2,
    VertexFormat__VERTEX_FORMAT_TEXTURED, VertexTextured,
};

use super::{LABELS, Label};

/// Stream one label's quad into the shared dynamic vertex buffer and draw it.
/// `Particle_DoRender` builds the four world-space verts from the camera
/// basis (`Gfx.View` rows), centered on the label anchor and raised by half
/// the quad height -- so the anchor is the bottom-center of the text.
fn draw_billboard(vb: GfxResourceID, label: &Label) {
    let tex = label.tex.as_texture();
    let size = Vec2 {
        x: label.size_world.0,
        y: label.size_world.1,
    };
    // Plain white; color codes baked into the texture still show through.
    let col = PackedCol_Make(255, 255, 255, 255);
    let verts = Particle_DoRender(&size, &label.anchor, &tex.uv, col);

    unsafe {
        Gfx_BindTexture(tex.ID);
        let dst = Gfx_LockDynamicVb(vb, VertexFormat__VERTEX_FORMAT_TEXTURED, 4);
        ptr::copy_nonoverlapping(verts.as_ptr(), dst.cast::<VertexTextured>(), 4);
        Gfx_UnlockDynamicVb(vb);
        Gfx_DrawVb_IndexedTris(4);
    }
}

/// Billboard draw pass: set the textured vertex format, then stream every
/// cached label quad into the shared VB and draw it. Returns immediately if
/// the VB is unavailable (GPU context lost or not yet created). Called by the
/// shared HUD render hook in `hud/render.rs` after the lines pass so labels
/// draw on top of the route ribbon.
///
/// The 3D state (depth/blend/proj/view) is set up by the caller before
/// invoking this pass, and the 2D-ortho state is restored by the caller
/// afterwards; this function only sets the vertex format and streams quads.
pub(in crate::plugin::hud) fn draw_pass() {
    let Some(vb) = super::context::vb_resource_id() else {
        return;
    };
    unsafe {
        Gfx_SetVertexFormat(VertexFormat__VERTEX_FORMAT_TEXTURED);
    }
    LABELS.with_borrow(|labels| {
        for label in labels {
            draw_billboard(vb, label);
        }
    });
}
