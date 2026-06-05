//! Shared HUD render hook: a single `OwnedScreen` at `HUD - 2` whose one
//! callback draws both the route-line and label-billboard passes in call
//! order (lines first, so the ribbon sits underneath the floating text).
//!
//! Owned by `HudModule`. Each pass sets its own vertex format and handles its
//! own VB-unavailable early return; the shared 3D-state setup and 2D-ortho
//! restore run once here, around both passes.

use std::ffi::c_void;

use classicube_sys::{
    Game, Gfx, Gfx_LoadMatrix, Gfx_SetAlphaBlending, Gfx_SetAlphaTest, Gfx_SetDepthTest,
    Gfx_SetDepthWrite, Matrix, MatrixType__MATRIX_PROJ, MatrixType__MATRIX_VIEW, OwnedScreen,
    screen::Priority,
};

use super::{labels, lines, shared};

/// Called from `Gui_RenderGui` between `Gfx_Begin2D` and the HUD screen's
/// render. Switches to 3D-style state, runs the route-line pass then the
/// label-billboard pass, then restores the 2D state the HUD (and later
/// screens) expect.
unsafe extern "C" fn render(_elem: *mut c_void, _delta: f32) {
    unsafe {
        if Gfx.LostContext != 0 {
            return;
        }

        // 3D-style state: depth test on (geometry behind walls is occluded),
        // depth write off (translucent quads shouldn't z-fight each other),
        // alpha blending on. Vertex format is NOT set here -- each pass sets
        // its own (COLOURED for lines, TEXTURED for labels).
        Gfx_SetDepthTest(1);
        Gfx_SetDepthWrite(0);
        Gfx_SetAlphaBlending(1);
        Gfx_LoadMatrix(MatrixType__MATRIX_PROJ, &raw const Gfx.Projection);
        Gfx_LoadMatrix(MatrixType__MATRIX_VIEW, &raw const Gfx.View);

        // Lines first (underneath), labels second (on top). Each pass handles
        // its own VB-unavailable early return independently.
        lines::draw_pass();
        labels::draw_pass();

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

/// Build and register the shared HUD render hook, returning the `OwnedScreen`
/// for `HudModule` to own. The `OwnedScreen` boxes its `Screen` + vtable, so
/// it stays valid after being moved into the module field; dropping it calls
/// `Gui_Remove`.
pub(super) fn install() -> OwnedScreen {
    let mut screen = OwnedScreen::new();
    screen.on_render(render);
    // One slot below the under-HUD layer, where the sibling chat-bubbles
    // plugin also registers (at UnderHud = HUD - 1 = 9). ClassiCube's Gui_Add
    // allows one screen per priority and evicts the incumbent, so sharing
    // UnderHud would make whichever plugin loads second clobber the other's
    // render hook. Sit at UnderHud - 1 (= HUD - 2 = 8): still under the HUD
    // (chatbox / hotbar / crosshair draw on top) and under the bubbles, but
    // its own slot so both coexist regardless of load order. Nameplates
    // already drew in the 3D phase, so labels and lines still land above them.
    screen.add(Priority::Custom(Priority::UnderHud.to_u8() - 1));
    screen
}
