//! 2D screen-space draw pass for the built-in timer overlay.
//!
//! Reimplements `Gfx_Draw2DTexture`/`Gfx_Make2DQuad` (not `CC_API`, so
//! not bound in classicube-sys) following the pattern from
//! `classicube-chat-bubbles-plugin/src/plugin/rendering/context/vertex_buffer.rs`.

use std::{cell::RefCell, ffi::c_void, ptr};

use classicube_sys::{
    Game, Gfx, Gfx_BindTexture, Gfx_DrawVb_IndexedTris, Gfx_LoadMatrix, Gfx_LockDynamicVb,
    Gfx_SetAlphaBlending, Gfx_SetAlphaTest, Gfx_SetDepthTest, Gfx_SetDepthWrite,
    Gfx_SetVertexFormat, Gfx_UnlockDynamicVb, GfxResourceID, Matrix, MatrixType__MATRIX_PROJ,
    MatrixType__MATRIX_VIEW, OwnedScreen, OwnedTexture, PackedCol_Make, Texture,
    VertexFormat__VERTEX_FORMAT_TEXTURED, VertexTextured, screen::Priority,
};
use tracing::debug;

use super::{TIMER_STATE, context, format::format_time, texture};
use crate::plugin::{
    splits::{
        self,
        geometry::{CheckpointKind, kind_color_code},
    },
    timer::state::Phase,
};

/// Mirror of `hud/shared.rs:calc_ortho_matrix`, needed here because
/// `hud::shared` is a private module. Keeps the timer overlay independent
/// of the HUD module's internals.
fn calc_ortho_matrix(width: f32, height: f32, z_near: f32, z_far: f32) -> Matrix {
    let mut m = Matrix::IDENTITY;
    m.row1.x = 2.0 / width;
    m.row2.y = -2.0 / height;
    if cfg!(target_os = "windows") {
        let adjust_x = 0.5 * (2.0 / width);
        let adjust_y = 0.5 * (-2.0 / height);
        m.row3.z = 1.0 / (z_near - z_far);
        m.row4.x = -1.0 - adjust_x;
        m.row4.y = 1.0 - adjust_y;
        m.row4.z = z_near / (z_near - z_far);
    } else {
        m.row3.z = -2.0 / (z_far - z_near);
        m.row4.x = -1.0;
        m.row4.y = 1.0;
        m.row4.z = -(z_far + z_near) / (z_far - z_near);
    }
    m
}

const MARGIN: i16 = 8;
const ROW_GAP: i16 = 2;

thread_local! {
    /// Cached texture for the clock line, keyed by the formatted string.
    static CLOCK_TEX: RefCell<Option<(String, OwnedTexture)>> = const { RefCell::new(None) };

    /// Cached textures for the split rows, keyed by the row content vector.
    /// Rebuilt whenever any split fires, is undone, or the run resets.
    static SPLIT_TEXTURES: RefCell<Vec<Option<OwnedTexture>>> = const { RefCell::new(Vec::new()) };
    /// The `(kind, label, time_string)` set `SPLIT_TEXTURES` was built from.
    static LAST_SPLIT_KEY: RefCell<Vec<(CheckpointKind, String, Option<String>)>> = const { RefCell::new(Vec::new()) };
}

pub fn invalidate() {
    CLOCK_TEX.with_borrow_mut(|c| *c = None);
    SPLIT_TEXTURES.with_borrow_mut(Vec::clear);
    LAST_SPLIT_KEY.with_borrow_mut(Vec::clear);
}

/// Build the render hook and register it at `Priority::OverHud` so the
/// timer clock draws above the engine's HUD (hotbar, chat).
pub fn install() -> OwnedScreen {
    let mut screen = OwnedScreen::new();
    screen.on_render(render);
    screen.add(Priority::OverHud);
    screen
}

unsafe extern "C" fn render(_elem: *mut c_void, _delta: f32) {
    unsafe {
        if Gfx.LostContext != 0 {
            return;
        }
        let Some(vb) = context::vb_resource_id() else {
            return;
        };

        // Restore 2D ortho state defensively: the HUD screen at a lower
        // priority may have left us in 3D state. Gfx_Begin2D already did
        // this, but other OwnedScreen hooks can clobber it.
        #[expect(
            clippy::cast_precision_loss,
            reason = "window dimensions are small positive ints"
        )]
        let ortho = calc_ortho_matrix(Game.Width as f32, Game.Height as f32, -100.0, 1000.0);
        Gfx_LoadMatrix(MatrixType__MATRIX_PROJ, &ortho);
        Gfx_LoadMatrix(MatrixType__MATRIX_VIEW, &Matrix::IDENTITY);
        Gfx_SetAlphaBlending(1);
        Gfx_SetDepthTest(0);
        Gfx_SetDepthWrite(0);
        Gfx_SetAlphaTest(0);
        Gfx_SetVertexFormat(VertexFormat__VERTEX_FORMAT_TEXTURED);

        if !super::SHOW.get() {
            return;
        }
        draw_overlay(vb);
    }
}

fn draw_overlay(vb: GfxResourceID) {
    TIMER_STATE.with_borrow(|slot| {
        let Some(state) = slot.as_ref() else {
            return;
        };

        let clock = super::clock();
        let elapsed = state.elapsed_now(clock);

        // --- Clock line ---
        let clock_str = format_time(elapsed);
        let clock_color = phase_color(state.phase);

        // Rebuild clock texture if the string changed (happens every frame
        // while Running since the millisecond digits update continuously).
        let needs_clock_rebuild =
            CLOCK_TEX.with_borrow(|slot| slot.as_ref().is_none_or(|(s, _)| s != &clock_str));
        if needs_clock_rebuild {
            let new_tex = texture::create_clock_texture(&clock_str);
            CLOCK_TEX.with_borrow_mut(|slot| {
                *slot = new_tex.map(|t| (clock_str.clone(), t));
            });
        }

        let clock_height = CLOCK_TEX.with_borrow(|slot| {
            if let Some((_, tex)) = slot.as_ref() {
                draw_texture_at(vb, tex, MARGIN, MARGIN, clock_color);
                tex.as_texture().height as i16
            } else {
                0
            }
        });

        // --- Split rows ---
        // Pre-run (NotRunning, or after a Reset) the state's `split_rows` are
        // empty -- they're only populated at `Command::Start`. Derive the row
        // list from the loaded track instead, so the split list is visible on
        // track load with blank times. Once a run begins, the state's rows are
        // the source of truth (they carry the captured per-split times) and
        // also reflect live editor edits made before Start.
        let split_key: Vec<(CheckpointKind, String, Option<String>)> =
            if state.split_rows.is_empty() {
                splits::checkpoint_rows()
                    .into_iter()
                    .map(|(kind, label)| (kind, label, None))
                    .collect()
            } else {
                state
                    .split_rows
                    .iter()
                    .map(|row| (row.kind, row.label.clone(), row.time.map(format_time)))
                    .collect()
            };

        let needs_rebuild =
            LAST_SPLIT_KEY.with_borrow(|last| last.as_slice() != split_key.as_slice());
        if needs_rebuild {
            debug!("rebuilding timer split row textures");
            let textures: Vec<Option<OwnedTexture>> = split_key
                .iter()
                .map(|(kind, label, time_opt)| {
                    let time_str = time_opt.as_deref().unwrap_or("--:--.---");
                    let code = kind_color_code(*kind);
                    let text = if label.is_empty() {
                        format!("{code}{time_str}")
                    } else {
                        format!("{code}{label}  &f{time_str}")
                    };
                    texture::create_split_texture(&text)
                })
                .collect();
            SPLIT_TEXTURES.with_borrow_mut(|slot| *slot = textures);
            LAST_SPLIT_KEY.with_borrow_mut(|last| *last = split_key);
        }

        let mut y = MARGIN + clock_height + ROW_GAP;
        SPLIT_TEXTURES.with_borrow(|textures| {
            for tex in textures.iter().flatten() {
                let row_h = tex.as_texture().height as i16;
                draw_texture_at(vb, tex, MARGIN, y, PackedCol_Make(255, 255, 255, 220));
                y += row_h + ROW_GAP;
            }
        });
    });
}

fn phase_color(phase: Phase) -> u32 {
    match phase {
        Phase::NotRunning => PackedCol_Make(200, 200, 200, 255),
        Phase::Running => PackedCol_Make(0, 255, 80, 255),
        Phase::Ended => PackedCol_Make(255, 200, 0, 255),
    }
}

/// Draw a texture positioned at `(x, y)` screen pixels with the given color.
fn draw_texture_at(vb: GfxResourceID, tex: &OwnedTexture, x: i16, y: i16, color: u32) {
    // SAFETY: The OwnedTexture outlives this stack frame; get_texture returns
    // a value copy via struct spread so the GPU id stays valid.
    let mut t = unsafe { tex.get_texture() };
    t.x = x;
    t.y = y;
    unsafe {
        Gfx_BindTexture(t.ID);
        let verts = make_2d_quad(&t, color);
        let dst = Gfx_LockDynamicVb(vb, VertexFormat__VERTEX_FORMAT_TEXTURED, 4);
        ptr::copy_nonoverlapping(verts.as_ptr(), dst.cast::<VertexTextured>(), 4);
        Gfx_UnlockDynamicVb(vb);
        Gfx_DrawVb_IndexedTris(4);
    }
}

/// Build four counter-clockwise `VertexTextured` verts from a positioned
/// `Texture`. Mirrors `Gfx_Draw2DTexture` / `Gfx_Make2DQuad` from
/// `classicube-chat-bubbles-plugin/src/plugin/rendering/context/vertex_buffer.rs`.
fn make_2d_quad(tex: &Texture, color: u32) -> [VertexTextured; 4] {
    let x1 = f32::from(tex.x);
    let x2 = x1 + f32::from(tex.width);
    let y1 = f32::from(tex.y);
    let y2 = y1 + f32::from(tex.height);
    [
        VertexTextured {
            x: x1,
            y: y1,
            z: 0.0,
            Col: color,
            U: tex.uv.u1,
            V: tex.uv.v1,
        },
        VertexTextured {
            x: x2,
            y: y1,
            z: 0.0,
            Col: color,
            U: tex.uv.u2,
            V: tex.uv.v1,
        },
        VertexTextured {
            x: x2,
            y: y2,
            z: 0.0,
            Col: color,
            U: tex.uv.u2,
            V: tex.uv.v2,
        },
        VertexTextured {
            x: x1,
            y: y2,
            z: 0.0,
            Col: color,
            U: tex.uv.u1,
            V: tex.uv.v2,
        },
    ]
}
