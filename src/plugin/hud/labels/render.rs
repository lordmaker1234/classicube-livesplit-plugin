//! Camera-facing billboard renderer for the floating checkpoint labels.
//!
//! Installs an `OwnedScreen` render hook just below the HUD (priority
//! `HUD - 2`, so the chatbox / hotbar / crosshair still draw on top) and,
//! each frame, draws every cached label as a world-anchored billboard above
//! its box.
//!
//! The billboard math is the engine's name-tag technique: world-space verts
//! built from the camera's right/up basis (the `Gfx.View` rows), via the
//! `Particle_DoRender` helper -- the same path `EntityRenderers.c:DrawName`
//! and `Particle.c:Particle_DoRender` use. The 2D-ortho save/restore scaffold
//! and `calc_ortho_matrix` are ported from
//! `classicube-chat-bubbles-plugin/src/plugin/rendering/render_hook/mod.rs`.

use std::{ffi::c_void, ptr};

use classicube_sys::{
    Game, Gfx, Gfx_BindTexture, Gfx_DrawVb_IndexedTris, Gfx_LoadMatrix, Gfx_LockDynamicVb,
    Gfx_SetAlphaBlending, Gfx_SetAlphaTest, Gfx_SetDepthTest, Gfx_SetDepthWrite,
    Gfx_SetVertexFormat, Gfx_UnlockDynamicVb, GfxResourceID, Matrix, MatrixType__MATRIX_PROJ,
    MatrixType__MATRIX_VIEW, OwnedScreen, PackedCol_Make, Particle_DoRender, Vec2,
    VertexFormat__VERTEX_FORMAT_TEXTURED, VertexTextured, screen::Priority,
};

use super::{LABELS, Label, shared};

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

/// Called from `Gui_RenderGui` between `Gfx_Begin2D` and the HUD screen's
/// render. Switch to 3D-style state, draw the billboards, then restore the
/// 2D state the HUD (and later screens) expect.
unsafe extern "C" fn render(_elem: *mut c_void, _delta: f32) {
    unsafe {
        if Gfx.LostContext != 0 {
            return;
        }
        let Some(vb) = super::context::vb_resource_id() else {
            return;
        };

        // 3D-style state: depth test on (a label behind a wall is occluded),
        // depth write off (translucent text shouldn't write depth and
        // z-fight overlapping labels), alpha blending on.
        Gfx_SetDepthTest(1);
        Gfx_SetDepthWrite(0);
        Gfx_SetAlphaBlending(1);
        Gfx_LoadMatrix(MatrixType__MATRIX_PROJ, &raw const Gfx.Projection);
        Gfx_LoadMatrix(MatrixType__MATRIX_VIEW, &raw const Gfx.View);
        Gfx_SetVertexFormat(VertexFormat__VERTEX_FORMAT_TEXTURED);

        LABELS.with_borrow(|labels| {
            for label in labels {
                draw_billboard(vb, label);
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
        // alpha-test off; otherwise translucent HUD gradients (chat backdrop,
        // escape menu) get their <128-alpha pixels discarded.
        Gfx_SetAlphaTest(0);
    }
}

/// Build and register the billboard render hook, returning the `OwnedScreen`
/// for `LabelsModule` to own. The `OwnedScreen` boxes its `Screen` + vtable,
/// so it stays valid after being moved into the module field; dropping it
/// calls `Gui_Remove`.
pub(super) fn install() -> OwnedScreen {
    let mut screen = OwnedScreen::new();
    screen.on_render(render);
    // One slot below chat-bubbles, which also registers at UnderHud
    // (HUD - 1 = 9). ClassiCube's Gui_Add allows one screen per priority and
    // evicts the incumbent, so sharing UnderHud makes whichever plugin loads
    // second clobber the other's render hook. Sit at HUD - 2 (= 8): still
    // under the HUD (chatbox / hotbar / crosshair draw on top) and under the
    // bubbles, but its own slot so both coexist regardless of load order.
    // Nameplates already drew in the 3D phase, so labels still land above them.
    screen.add(Priority::Custom(Priority::Hud.to_u8() - 2));
    screen
}
