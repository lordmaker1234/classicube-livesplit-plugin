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

use super::{LABELS, Label};

/// Mirror ClassiCube's per-backend `Gfx_CalcOrthoMatrix`, picking the formula
/// at compile time. `Matrix::orthographic` is GL-flavored (clip-space z
/// `[-1, 1]`) -- feeding it to D3D9/D3D11 (clip-space z `[0, 1]`) puts every
/// 2D vertex outside the clip range and the rasterizer culls the entire HUD.
///
/// `Gfx_CalcOrthoMatrix` itself is not `CC_API` and so isn't exported from
/// `ClassiCube.dll` on Windows, so we replicate it here.
fn calc_ortho_matrix(width: f32, height: f32, z_near: f32, z_far: f32) -> Matrix {
    let mut m = Matrix::IDENTITY;
    m.row1.x = 2.0 / width;
    m.row2.y = -2.0 / height;

    if cfg!(target_os = "windows") {
        // D3D9 / D3D11: clip-space z is [0, 1]; D3D9 also wants a half-pixel
        // x/y nudge. Mirrors `Graphics_D3D9.c:756` (z math is identical to
        // `Graphics_D3D11.c:507`; the half-pixel nudge is harmless on D3D11).
        let adjust_x = 0.5 * (2.0 / width);
        let adjust_y = 0.5 * (-2.0 / height);
        m.row3.z = 1.0 / (z_near - z_far);
        m.row4.x = -1.0 - adjust_x;
        m.row4.y = 1.0 - adjust_y;
        m.row4.z = z_near / (z_near - z_far);
    } else {
        // GL clip-space z is [-1, 1]. Mirrors `_GLShared.h:289`.
        m.row3.z = -2.0 / (z_far - z_near);
        m.row4.x = -1.0;
        m.row4.y = 1.0;
        m.row4.z = -(z_far + z_near) / (z_far - z_near);
    }
    m
}

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
        // `calc_ortho_matrix`).
        #[expect(
            clippy::cast_precision_loss,
            reason = "window dimensions are small positive ints"
        )]
        let ortho = calc_ortho_matrix(Game.Width as f32, Game.Height as f32, -100.0, 1000.0);
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
