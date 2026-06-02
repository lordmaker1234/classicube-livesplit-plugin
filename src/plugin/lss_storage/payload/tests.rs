use classicube_sys::Vec3;

use super::*;
use crate::plugin::splits::geometry::{Aabb, Checkpoint, CheckpointKind, Trigger};

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
fn canonical_is_text_stable() {
    let t = sample_track();
    let a = serialize_canonical(&t).unwrap();
    let b = serialize_canonical(&t).unwrap();
    assert_eq!(a, b);
}

#[test]
fn serialized_text_is_readable_and_label_free() {
    let t = sample_track();
    let text = serialize_canonical(&t).unwrap();
    // Bare keyword lines only: the segment labels ("start", "lobby",
    // "end") never appear, there's no `LS label` line, and no inline
    // label rides after the coords / map name.
    assert_eq!(
        text,
        "LS v1\nLS title any%\nLS cp 64,40,128 2,3,2\nLS map AwesomeLobby\nLS cp 200,50,400 \
         4,2,4\nLS end"
    );
    assert!(text.starts_with("LS v1\n"));
    assert!(text.contains("LS title "));
    assert!(text.contains("LS cp "));
    assert!(text.contains("LS map "));
    assert!(text.ends_with("LS end"));
    assert!(!text.contains("LS label"));
}

#[test]
fn round_trip_via_parse_defaults_start_label() {
    let t = sample_track(); // Start label is "start".
    let text = serialize_canonical(&t).unwrap();
    let back = parse(&text, segment_labels_of(&t)).unwrap();

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
fn label_only_change_yields_equal_text() {
    let mut a = sample_track();
    let mut b = sample_track();
    for cp in &mut a.checkpoints {
        cp.label = format!("{}-A", cp.label);
    }
    for cp in &mut b.checkpoints {
        cp.label = format!("{}-B", cp.label);
    }
    assert_eq!(
        serialize_canonical(&a).unwrap(),
        serialize_canonical(&b).unwrap(),
        "labels must not affect canonical text"
    );
}

#[test]
fn aabb_min_change_yields_different_text() {
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
fn aabb_size_change_yields_different_text() {
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
fn map_target_change_yields_different_text() {
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
fn checkpoint_count_change_yields_different_text() {
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
fn track_name_change_yields_different_text() {
    let a = sample_track();
    let mut b = sample_track();
    b.name = "100%".to_owned();
    assert_ne!(
        serialize_canonical(&a).unwrap(),
        serialize_canonical(&b).unwrap()
    );
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
fn parse_substitutes_placeholder_for_empty_label() {
    // sample_track is [Start, Split(lobby), End]; segments are the two
    // non-start checkpoints. An empty first segment is the Split at
    // checkpoint index 1, so the placeholder is "split 1".
    let t = sample_track();
    let text = serialize_canonical(&t).unwrap();
    let labels = vec![String::new(), "end".into()];
    let back = parse(&text, labels).unwrap();
    assert_eq!(back.checkpoints[1].label, "split 1");
    // The Start always gets the default, regardless of segments.
    assert_eq!(back.checkpoints[0].label, "Start");
}

#[test]
fn parse_rejects_label_count_mismatch() {
    let t = sample_track(); // 3 checkpoints -> expects 2 segment labels.
    let text = serialize_canonical(&t).unwrap();
    // Too few.
    assert!(parse(&text, vec!["only".into()]).is_err());
    // Too many: a label per checkpoint (the pre-change format) is now a
    // mismatch, since the Start no longer has a segment.
    assert!(parse(&text, vec!["start".into(), "lobby".into(), "end".into()]).is_err());
}

#[test]
fn parse_rejects_legacy_non_ls_value() {
    // An old base64/postcard payload doesn't start with `LS `, so the
    // geometry decoder rejects it cleanly -- the clean-break behavior
    // (old files are skipped and regenerated by reload + save).
    assert!(parse("AQVhbnklAwAB", vec!["lobby".into(), "end".into()]).is_err());
}

#[test]
fn parse_tolerates_indentation_and_crlf() {
    // A hand-edited / XML-reflowed value with leading indentation on
    // each line and CRLF endings still decodes to the same geometry.
    let t = sample_track();
    let text = serialize_canonical(&t).unwrap();
    let messy = text
        .lines()
        .map(|l| format!("    {l}"))
        .collect::<Vec<_>>()
        .join("\r\n");
    let back = parse(&messy, segment_labels_of(&t)).unwrap();
    let clean = parse(&text, segment_labels_of(&t)).unwrap();
    assert_eq!(back, clean);
}
