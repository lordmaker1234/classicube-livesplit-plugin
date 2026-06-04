use classicube_sys::Vec3;

use super::*;
use crate::plugin::splits::geometry::{Aabb, CheckpointKind};

fn make_aabb(min: (f32, f32, f32), max: (f32, f32, f32)) -> Aabb {
    Aabb {
        min: Vec3::create(min.0, min.1, min.2),
        max: Vec3::create(max.0, max.1, max.2),
    }
}

// --- label_anchor ---

#[test]
fn label_anchor_x_z_are_aabb_center() {
    let aabb = make_aabb((2.0, 0.0, 4.0), (6.0, 3.0, 10.0));
    let anchor = label_anchor(&aabb);
    assert_eq!(anchor.x, (aabb.min.x + aabb.max.x) / 2.0); // center x
    assert_eq!(anchor.z, (aabb.min.z + aabb.max.z) / 2.0); // center z
}

#[test]
fn label_anchor_y_is_above_top_face() {
    let aabb = make_aabb((0.0, 0.0, 0.0), (1.0, 5.0, 1.0));
    let anchor = label_anchor(&aabb);
    assert!(anchor.y > aabb.max.y);
    assert_eq!(anchor.y, aabb.max.y + LABEL_Y_OFFSET);
}

#[test]
fn label_anchor_unit_box() {
    // A 1x1x1 block at the origin: anchor should float above its center.
    let aabb = make_aabb((0.0, 0.0, 0.0), (1.0, 1.0, 1.0));
    let anchor = label_anchor(&aabb);
    assert_eq!(anchor.x, 0.5);
    assert_eq!(anchor.z, 0.5);
    assert!(anchor.y > 1.0);
}

// --- kind_rgb ---

#[test]
fn kind_rgb_start_is_green() {
    assert_eq!(kind_rgb(CheckpointKind::Start), (0, 255, 0));
}

#[test]
fn kind_rgb_split_is_yellow() {
    assert_eq!(kind_rgb(CheckpointKind::Split), (255, 255, 0));
}

#[test]
fn kind_rgb_pause_is_cyan() {
    assert_eq!(kind_rgb(CheckpointKind::Pause), (0, 200, 255));
}

#[test]
fn kind_rgb_resume_is_orange() {
    assert_eq!(kind_rgb(CheckpointKind::Resume), (255, 140, 0));
}

#[test]
fn kind_rgb_end_is_red() {
    assert_eq!(kind_rgb(CheckpointKind::End), (255, 0, 0));
}

// Guard that each kind produces a non-zero color (no accidentally blank checkpoint).
#[test]
fn kind_rgb_all_kinds_are_distinct_and_nonzero() {
    let kinds = [
        CheckpointKind::Start,
        CheckpointKind::Split,
        CheckpointKind::Pause,
        CheckpointKind::Resume,
        CheckpointKind::End,
    ];
    let rgbs: Vec<(u8, u8, u8)> = kinds.iter().map(|k| kind_rgb(*k)).collect();
    // All non-zero (no accidentally invisible color).
    for (r, g, b) in &rgbs {
        assert!(r | g | b != 0, "kind_rgb returned (0,0,0)");
    }
    // All distinct.
    for i in 0..rgbs.len() {
        for j in (i + 1)..rgbs.len() {
            assert_ne!(rgbs[i], rgbs[j], "two kinds share the same RGB");
        }
    }
}
