use classicube_sys::Vec3;

use super::*;
use crate::plugin::splits::geometry::{Aabb, Checkpoint, CheckpointKind, Track, Trigger};

fn aabb(min: (u16, u16, u16), max: (u16, u16, u16)) -> Aabb {
    Aabb {
        min: Vec3::new(f32::from(min.0), f32::from(min.1), f32::from(min.2)),
        max: Vec3::new(f32::from(max.0), f32::from(max.1), f32::from(max.2)),
    }
}

fn sample_track() -> Track {
    Track {
        name: "any%".to_owned(),
        checkpoints: vec![
            Checkpoint {
                kind: CheckpointKind::Start,
                trigger: Trigger::Aabb(aabb((64, 40, 128), (66, 43, 130))),
                label: "start".to_owned(),
            },
            Checkpoint {
                kind: CheckpointKind::Split,
                trigger: Trigger::MapLoaded("AwesomeLobby".to_owned()),
                label: "lobby".to_owned(),
            },
            Checkpoint {
                kind: CheckpointKind::End,
                trigger: Trigger::Aabb(aabb((200, 50, 400), (204, 52, 404))),
                label: "end".to_owned(),
            },
        ],
    }
}

/// Segment labels written to the `.lss`: every checkpoint except the
/// implicit Start at index 0 (which has no `<Segment>`).
fn segment_labels_of(t: &Track) -> Vec<String> {
    t.checkpoints
        .iter()
        .skip(1)
        .map(|c| c.label.clone())
        .collect()
}

#[test]
fn canonical_is_byte_stable() {
    let t = sample_track();
    let a = serialize_canonical(&t).unwrap();
    let b = serialize_canonical(&t).unwrap();
    assert_eq!(a, b);
}

#[test]
fn round_trip_preserves_non_start_and_defaults_start_label() {
    let t = sample_track(); // Start label is "start".
    let bytes = serialize_canonical(&t).unwrap();
    let payload = parse(&bytes).unwrap();
    let back = into_track(payload, segment_labels_of(&t)).unwrap();

    // Geometry, kinds, and non-start labels round-trip exactly.
    assert_eq!(back.name, t.name);
    assert_eq!(back.checkpoints.len(), t.checkpoints.len());
    for i in 1..t.checkpoints.len() {
        assert_eq!(back.checkpoints[i], t.checkpoints[i]);
    }

    // The Start has no segment, so its label isn't persisted: it comes
    // back as the default, but its kind and trigger survive.
    let start = &back.checkpoints[0];
    assert_eq!(start.kind, CheckpointKind::Start);
    assert_eq!(start.trigger, t.checkpoints[0].trigger);
    assert_eq!(start.label, "Start");
}

#[test]
fn label_only_change_yields_equal_bytes() {
    let mut a = sample_track();
    let mut b = sample_track();
    for cp in &mut a.checkpoints {
        cp.label = format!("{}-A", cp.label);
    }
    for cp in &mut b.checkpoints {
        cp.label = format!("{}-B", cp.label);
    }
    let ba = serialize_canonical(&a).unwrap();
    let bb = serialize_canonical(&b).unwrap();
    assert_eq!(ba, bb, "labels must not affect canonical bytes");
}

#[test]
fn aabb_min_change_yields_different_bytes() {
    let a = sample_track();
    let mut b = sample_track();
    if let Trigger::Aabb(ref mut bb) = b.checkpoints[0].trigger {
        bb.min.x += 1.0;
        bb.max.x += 1.0;
    }
    assert_ne!(
        serialize_canonical(&a).unwrap(),
        serialize_canonical(&b).unwrap()
    );
}

#[test]
fn aabb_size_change_yields_different_bytes() {
    let a = sample_track();
    let mut b = sample_track();
    if let Trigger::Aabb(ref mut bb) = b.checkpoints[0].trigger {
        bb.max.x += 1.0;
    }
    assert_ne!(
        serialize_canonical(&a).unwrap(),
        serialize_canonical(&b).unwrap()
    );
}

#[test]
fn map_target_change_yields_different_bytes() {
    let a = sample_track();
    let mut b = sample_track();
    if let Trigger::MapLoaded(ref mut name) = b.checkpoints[1].trigger {
        *name = "OtherLobby".to_owned();
    }
    assert_ne!(
        serialize_canonical(&a).unwrap(),
        serialize_canonical(&b).unwrap()
    );
}

#[test]
fn checkpoint_count_change_yields_different_bytes() {
    let a = sample_track();
    let mut b = sample_track();
    let extra = Checkpoint {
        kind: CheckpointKind::Split,
        trigger: Trigger::Aabb(aabb((150, 50, 250), (152, 53, 252))),
        label: "extra".to_owned(),
    };
    b.checkpoints.insert(2, extra);
    // Adjust kinds to be a valid sequence (last is End).
    b.checkpoints[2].kind = CheckpointKind::Split;
    b.checkpoints[3].kind = CheckpointKind::End;
    assert_ne!(
        serialize_canonical(&a).unwrap(),
        serialize_canonical(&b).unwrap()
    );
}

#[test]
fn track_name_change_yields_different_bytes() {
    let a = sample_track();
    let mut b = sample_track();
    b.name = "100%".to_owned();
    assert_ne!(
        serialize_canonical(&a).unwrap(),
        serialize_canonical(&b).unwrap()
    );
}

#[test]
fn rejects_unknown_schema_version() {
    let v2 = br#"{"v":2,"name":"x","checkpoints":[]}"#;
    assert!(parse(v2).is_err());
}

#[test]
fn rejects_too_few_checkpoints_on_serialize() {
    let t = Track {
        name: "x".to_owned(),
        checkpoints: vec![Checkpoint {
            kind: CheckpointKind::Start,
            trigger: Trigger::Aabb(aabb((0, 0, 0), (1, 1, 1))),
            label: "s".to_owned(),
        }],
    };
    assert!(serialize_canonical(&t).is_err());
}

#[test]
fn rejects_bad_kind_sequence_on_serialize() {
    let mut t = sample_track();
    t.checkpoints[0].kind = CheckpointKind::Split;
    assert!(serialize_canonical(&t).is_err());
}

#[test]
fn into_track_substitutes_placeholder_for_empty_label() {
    // sample_track is [Start, Split(lobby), End]; segments are the two
    // non-start checkpoints. An empty first segment is the Split at
    // checkpoint index 1, so the placeholder is "split 1".
    let t = sample_track();
    let bytes = serialize_canonical(&t).unwrap();
    let payload = parse(&bytes).unwrap();
    let labels = vec![String::new(), "end".into()];
    let back = into_track(payload, labels).unwrap();
    assert_eq!(back.checkpoints[1].label, "split 1");
    // The Start always gets the default, regardless of segments.
    assert_eq!(back.checkpoints[0].label, "Start");
}

#[test]
fn into_track_rejects_label_count_mismatch() {
    let t = sample_track(); // 3 checkpoints -> expects 2 segment labels.
    let bytes = serialize_canonical(&t).unwrap();
    let payload = parse(&bytes).unwrap();
    // Too few.
    assert!(into_track(payload, vec!["only".into()]).is_err());
    // Too many: a label per checkpoint (the pre-change format) is now a
    // mismatch, since the Start no longer has a segment.
    assert!(
        into_track(
            parse(&bytes).unwrap(),
            vec!["start".into(), "lobby".into(), "end".into()]
        )
        .is_err()
    );
}
