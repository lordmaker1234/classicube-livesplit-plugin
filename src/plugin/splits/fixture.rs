use classicube_sys::Vec3;

use crate::plugin::splits::geometry::{Aabb, Checkpoint, CheckpointKind, Track};

/// A small fixed track used by the `/client LiveSplit loadtest` chat
/// subcommand for development. Four checkpoints (Start, two Splits, End)
/// laid out along the +X axis at the world origin so a runner can walk
/// the line and exercise the full IPC path before a real track-source
/// is implemented.
#[must_use]
pub fn loadtest() -> Track {
    Track {
        name: "loadtest".into(),
        checkpoints: vec![
            checkpoint(
                CheckpointKind::Start,
                (0.0, 0.0, 0.0),
                (2.0, 4.0, 2.0),
                "start",
            ),
            checkpoint(
                CheckpointKind::Split,
                (10.0, 0.0, 0.0),
                (12.0, 4.0, 2.0),
                "split 1",
            ),
            checkpoint(
                CheckpointKind::Split,
                (20.0, 0.0, 0.0),
                (22.0, 4.0, 2.0),
                "split 2",
            ),
            checkpoint(
                CheckpointKind::End,
                (30.0, 0.0, 0.0),
                (32.0, 4.0, 2.0),
                "end",
            ),
        ],
    }
}

fn checkpoint(
    kind: CheckpointKind,
    min: (f32, f32, f32),
    max: (f32, f32, f32),
    label: &str,
) -> Checkpoint {
    Checkpoint {
        kind,
        aabb: Aabb {
            min: Vec3::new(min.0, min.1, min.2),
            max: Vec3::new(max.0, max.1, max.2),
        },
        label: Some(label.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loadtest_has_expected_kind_sequence() {
        let t = loadtest();
        let kinds: Vec<_> = t.checkpoints.iter().map(|c| c.kind).collect();
        assert_eq!(
            kinds,
            vec![
                CheckpointKind::Start,
                CheckpointKind::Split,
                CheckpointKind::Split,
                CheckpointKind::End,
            ]
        );
    }

    #[test]
    fn loadtest_labels_are_populated() {
        let t = loadtest();
        for cp in &t.checkpoints {
            assert!(cp.label.is_some());
        }
    }
}
