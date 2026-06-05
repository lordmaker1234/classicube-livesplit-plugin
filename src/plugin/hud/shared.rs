//! Palette and geometry helpers shared across all three HUD layers (boxes,
//! labels, lines) so the checkpoint colors and label-anchor math have a
//! single source of truth.
//!
//! All items here are `pub(super)` (visible throughout `hud` and its children
//! via `super::shared::*`).

#[cfg(test)]
mod tests;

use classicube_sys::{Matrix, Vec3};

use crate::plugin::splits::geometry::{Aabb, CheckpointKind};

/// Vertical gap (in blocks) between the top of a checkpoint box and the
/// bottom of its floating label, so the text clears the cuboid's top face.
/// Also used as the route-line endpoint height above each box.
pub(super) const LABEL_Y_OFFSET: f32 = 0.3;

/// Compute the label anchor for an AABB: the top-center of the box raised by
/// [`LABEL_Y_OFFSET`]. This is the bottom-center of the floating text
/// billboard *and* the endpoint the route-line segments connect.
pub(super) fn label_anchor(aabb: &Aabb) -> Vec3 {
    Vec3::create(
        (aabb.min.x + aabb.max.x) / 2.0,
        aabb.max.y + LABEL_Y_OFFSET,
        (aabb.min.z + aabb.max.z) / 2.0,
    )
}

/// RGB triple for a checkpoint kind: the single source of truth for the
/// palette shared by boxes, labels, and route lines. The caller applies its
/// own alpha so each layer can use a different opacity.
///
/// Hues: Start = green, Split = yellow, Pause = cyan, Resume = orange,
/// End = red.
pub(super) fn kind_rgb(kind: CheckpointKind) -> (u8, u8, u8) {
    match kind {
        CheckpointKind::Start => (0, 255, 0),
        CheckpointKind::Split => (255, 255, 0),
        CheckpointKind::Pause => (0, 200, 255),
        CheckpointKind::Resume => (255, 140, 0),
        CheckpointKind::End => (255, 0, 0),
    }
}

/// Mirror ClassiCube's per-backend `Gfx_CalcOrthoMatrix`, picking the formula
/// at compile time. `Matrix::orthographic` is GL-flavored (clip-space z
/// `[-1, 1]`) -- feeding it to D3D9/D3D11 (clip-space z `[0, 1]`) puts every
/// 2D vertex outside the clip range and the rasterizer culls the entire HUD.
///
/// `Gfx_CalcOrthoMatrix` itself is not `CC_API` and so isn't exported from
/// `ClassiCube.dll` on Windows, so we replicate it here.
///
/// Used by the shared HUD render hook (`hud/render.rs`) to restore the 2D
/// orthographic state after the 3D drawing passes.
pub(super) fn calc_ortho_matrix(width: f32, height: f32, z_near: f32, z_far: f32) -> Matrix {
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
