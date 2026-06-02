use classicube_sys::Vec3;
use livesplit_core::run::parser::livesplit::parse;

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
                kind: CheckpointKind::End,
                trigger: Trigger::Aabb(aabb((200, 50, 400), (204, 52, 404))),
                label: "end".to_owned(),
            },
        ],
    }
}

fn unique_tmp_dir(prefix: &str) -> std::path::PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("lss-{prefix}-{}-{}", std::process::id(), n));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn build_lss_xml_stores_canonical_text() {
    let t = sample_track();
    let canonical = payload::serialize_canonical(&t).unwrap();

    let xml = build_lss_xml(&t, "MyServer", &canonical).unwrap();
    let run = parse(&xml).expect("re-parse");

    let stored = run
        .metadata()
        .custom_variable_value(CUSTOM_VAR_NAME)
        .expect("ClassiCubeTrack present");
    // The stored value is the readable `LS …` geometry text verbatim.
    assert_eq!(stored, canonical);

    // The Start checkpoint is the timer-side run-start action, not a
    // named split, so it's omitted from the segment list -- only "end"
    // remains.
    let names: Vec<&str> = run.segments().iter().map(|s| s.name()).collect();
    assert_eq!(names, vec!["end"]);

    assert_eq!(run.game_name(), "ClassiCube");
    assert!(run.category_name().contains("MyServer"));
    assert!(run.category_name().contains("any%"));
}

#[test]
fn build_lss_xml_round_trips_through_parse() {
    let t = sample_track(); // [Start "start", End "end"]
    let canonical = payload::serialize_canonical(&t).unwrap();

    let xml = build_lss_xml(&t, "Srv", &canonical).unwrap();
    let run = parse(&xml).expect("re-parse");

    let stored = run
        .metadata()
        .custom_variable_value(CUSTOM_VAR_NAME)
        .expect("ClassiCubeTrack present");
    let labels: Vec<String> = run.segments().iter().map(|s| s.name().to_owned()).collect();
    let back = payload::parse(stored, labels).unwrap();

    assert_eq!(back.checkpoints.len(), 2);
    // Start: kind + geometry survive; label defaults (no segment).
    assert_eq!(back.checkpoints[0].kind, CheckpointKind::Start);
    assert_eq!(back.checkpoints[0].trigger, t.checkpoints[0].trigger);
    assert_eq!(back.checkpoints[0].label, "Start");
    // End: its segment carries the label through.
    assert_eq!(back.checkpoints[1], t.checkpoints[1]);
}

#[test]
fn build_lss_xml_strips_color_codes_from_display() {
    let mut t = sample_track();
    t.name = "&aany%".to_owned();
    let canonical = payload::serialize_canonical(&t).unwrap();

    let xml = build_lss_xml(&t, "&cMy&eServer", &canonical).unwrap();
    let run = parse(&xml).unwrap();
    let cat = run.category_name();
    assert!(
        !cat.contains('&'),
        "category name still has color code: {cat}"
    );
    assert!(cat.contains("MyServer"));
    assert!(cat.contains("any%"));
}

#[test]
fn track_name_with_color_code_round_trips_through_payload() {
    // `&` in the track name lands in the `LS title` line. livesplit-core
    // XML-escapes the custom-variable text on save and unescapes it on
    // load, so the color code survives the round-trip intact (no manual
    // escaping needed on our side).
    let mut t = sample_track();
    t.name = "&aany%".to_owned();
    let canonical = payload::serialize_canonical(&t).unwrap();
    assert!(canonical.starts_with("LS title &aany%"));

    let xml = build_lss_xml(&t, "Srv", &canonical).unwrap();
    let run = parse(&xml).expect("re-parse");
    let stored = run
        .metadata()
        .custom_variable_value(CUSTOM_VAR_NAME)
        .expect("ClassiCubeTrack present");
    assert_eq!(stored, canonical);

    let labels: Vec<String> = run.segments().iter().map(|s| s.name().to_owned()).collect();
    let back = payload::parse(stored, labels).unwrap();
    assert_eq!(back.name, "&aany%");
}

#[test]
fn save_track_to_writes_then_dedups() {
    let dir = unique_tmp_dir("write-dedup");
    let category = "anypct";

    let t = sample_track();
    match save_track_to(&t, "Srv", &dir, category).unwrap() {
        SaveOutcome::Wrote(p) => {
            assert_eq!(
                p.file_name().unwrap(),
                format!("{category}-v1.lss").as_str()
            );
        }
        SaveOutcome::AlreadyLatest => panic!("first call should have written"),
    }

    assert!(matches!(
        save_track_to(&t, "Srv", &dir, category).unwrap(),
        SaveOutcome::AlreadyLatest
    ));

    let mut t2 = sample_track();
    if let Trigger::Aabb(ref mut bb) = t2.checkpoints[0].trigger {
        bb.min.x += 1.0;
        bb.max.x += 1.0;
    }
    match save_track_to(&t2, "Srv", &dir, category).unwrap() {
        SaveOutcome::Wrote(p) => {
            assert_eq!(
                p.file_name().unwrap(),
                format!("{category}-v2.lss").as_str()
            );
        }
        SaveOutcome::AlreadyLatest => panic!("modified track should have written"),
    }

    // Label-only change: the geometry text is identical (labels live in
    // the `<Segment>` elements, not the payload), so the dedup gate still
    // reports no change -- the no-bump-on-relabel guarantee.
    let mut t3 = t2.clone();
    t3.checkpoints[0].label = "renamed".to_owned();
    assert!(matches!(
        save_track_to(&t3, "Srv", &dir, category).unwrap(),
        SaveOutcome::AlreadyLatest
    ));

    let _ = std::fs::remove_dir_all(&dir);
}
