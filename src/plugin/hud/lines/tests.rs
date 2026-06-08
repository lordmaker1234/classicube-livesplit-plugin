use classicube_sys::Vec3;

use super::*;
use crate::plugin::splits::geometry::{Aabb, CheckpointKind};

fn make_aabb(min: (f32, f32, f32), max: (f32, f32, f32)) -> Aabb {
    Aabb {
        min: Vec3::create(min.0, min.1, min.2),
        max: Vec3::create(max.0, max.1, max.2),
    }
}

fn make_visible(kind: CheckpointKind, aabb: Aabb) -> (usize, CheckpointKind, Aabb, String, bool) {
    (0, kind, aabb, String::new(), false)
}

// ---- cross product ----

#[test]
fn cross_x_hat_y_hat_gives_z_hat() {
    let x_hat = Vec3::create(1.0, 0.0, 0.0);
    let y_hat = Vec3::create(0.0, 1.0, 0.0);
    let result = cross(x_hat, y_hat);
    assert_eq!(result.x, 0.0);
    assert_eq!(result.y, 0.0);
    assert_eq!(result.z, 1.0);
}

#[test]
fn cross_y_hat_x_hat_gives_neg_z_hat() {
    let x_hat = Vec3::create(1.0, 0.0, 0.0);
    let y_hat = Vec3::create(0.0, 1.0, 0.0);
    let result = cross(y_hat, x_hat);
    assert_eq!(result.x, 0.0);
    assert_eq!(result.y, 0.0);
    assert_eq!(result.z, -1.0);
}

#[test]
fn cross_parallel_vectors_is_zero() {
    let u = Vec3::create(1.0, 0.0, 0.0);
    let v = Vec3::create(2.0, 0.0, 0.0);
    let result = cross(u, v);
    assert_eq!(result.x, 0.0);
    assert_eq!(result.y, 0.0);
    assert_eq!(result.z, 0.0);
}

// ---- derive_waypoints ----

#[test]
fn derive_waypoints_empty_visible_gives_empty() {
    let waypoints = derive_waypoints(&[]);
    assert!(waypoints.is_empty());
}

#[test]
fn derive_waypoints_preserves_order_and_kinds() {
    let aabb = make_aabb((0.0, 0.0, 0.0), (2.0, 2.0, 2.0));
    let visible = vec![
        make_visible(CheckpointKind::Start, aabb),
        make_visible(CheckpointKind::Split, aabb),
        make_visible(CheckpointKind::End, aabb),
    ];
    let wps = derive_waypoints(&visible);
    assert_eq!(wps.len(), 3);
    assert_eq!(wps[0].1, CheckpointKind::Start);
    assert_eq!(wps[1].1, CheckpointKind::Split);
    assert_eq!(wps[2].1, CheckpointKind::End);
}

#[test]
fn derive_waypoints_anchor_is_top_center_of_aabb() {
    let aabb = make_aabb((0.0, 0.0, 0.0), (4.0, 3.0, 6.0));
    let visible = vec![make_visible(CheckpointKind::Split, aabb)];
    let wps = derive_waypoints(&visible);
    assert_eq!(wps.len(), 1);
    let anchor = wps[0].0;
    // x and z are the center of the AABB.
    assert_eq!(anchor.x, (aabb.min.x + aabb.max.x) / 2.0);
    assert_eq!(anchor.z, (aabb.min.z + aabb.max.z) / 2.0);
    // y is above the top face.
    assert!(anchor.y > aabb.max.y);
    assert_eq!(anchor.y, aabb.max.y + shared::LABEL_Y_OFFSET);
}

#[test]
fn derive_waypoints_caps_at_hud_id_count() {
    let aabb = make_aabb((0.0, 0.0, 0.0), (1.0, 1.0, 1.0));
    // Build more entries than HUD_ID_COUNT.
    let visible: Vec<_> = (0..=HUD_ID_COUNT)
        .map(|i| (i, CheckpointKind::Split, aabb, String::new(), false))
        .collect();
    let wps = derive_waypoints(&visible);
    assert_eq!(wps.len(), HUD_ID_COUNT);
}

// ---- segment pairing (windows(2) behavior on waypoints) ----

#[test]
fn fewer_than_two_waypoints_yields_no_segments() {
    // 0 waypoints
    let wps: Vec<(Vec3, CheckpointKind)> = vec![];
    assert_eq!(wps.windows(2).count(), 0);

    // 1 waypoint
    let wps = [(Vec3::create(0.0, 0.0, 0.0), CheckpointKind::Start)];
    assert_eq!(wps.windows(2).count(), 0);
}

#[test]
fn segment_uses_destination_kind() {
    // Three waypoints -> two segments.
    // Segment 0->1 uses kind of waypoint 1 (Split).
    // Segment 1->2 uses kind of waypoint 2 (End).
    let wps = [
        (Vec3::create(0.0, 0.0, 0.0), CheckpointKind::Start),
        (Vec3::create(1.0, 0.0, 0.0), CheckpointKind::Split),
        (Vec3::create(2.0, 0.0, 0.0), CheckpointKind::End),
    ];
    let segment_kinds: Vec<CheckpointKind> = wps.windows(2).map(|w| w[1].1).collect();
    assert_eq!(
        segment_kinds,
        vec![CheckpointKind::Split, CheckpointKind::End]
    );
}

#[test]
fn two_waypoints_give_one_segment() {
    let wps = [
        (Vec3::create(0.0, 0.0, 0.0), CheckpointKind::Start),
        (Vec3::create(5.0, 0.0, 0.0), CheckpointKind::End),
    ];
    let segments: Vec<_> = wps.windows(2).collect();
    assert_eq!(segments.len(), 1);
    // The single segment's destination is End.
    assert_eq!(segments[0][1].1, CheckpointKind::End);
}
