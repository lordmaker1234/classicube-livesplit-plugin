use anyhow::{Result, bail, ensure};

use crate::plugin::splits::geometry::{CheckpointKind, Track, Trigger, aabb_to_min_size};

/// Maximum per-line length, in codepoints. ClassiCube's
/// `INPUTWIDGET_LEN`/`STRING_SIZE` wrap point is 64; subtract 3 for the
/// default color prefix the server prepends on echo, leaving 61 cp for
/// our payload. Going over means `LineWrapper` re-splits the line and
/// inserts a `> &X` continuation marker, which we don't reassemble on
/// the receive side.
pub(crate) const MAX_LINE_CP: usize = 64 - 3;

/// Encode a `Track` into a series of `LS …` chat lines. The caller is
/// responsible for emitting them — one per `/mb sign` block, or chained
/// into a single block via the `mb` arm of the command module.
///
/// Layout:
///   line[0]   = `LS title <name>`
///   line[1..] = per checkpoint, in order. AABB checkpoints emit
///               `LS start|cp|end <min> <size> [label]`, optionally
///               followed by `LS label <text>` if the label overflowed
///               inline. Map-loaded checkpoints emit
///               `LS map|endmap <name>` bare, always followed by
///               `LS label <text>` (the `<name>` slot consumes to
///               end-of-line, leaving no inline label slot).
pub fn encode_for_chat(track: &Track) -> Result<Vec<String>> {
    let n = track.checkpoints.len();
    ensure!(
        n >= 2,
        "track has {n} checkpoint(s); need at least 2 (Start + End)"
    );
    ensure!(!track.name.trim().is_empty(), "track name is empty");

    for (i, cp) in track.checkpoints.iter().enumerate() {
        let expected = if i == 0 {
            CheckpointKind::Start
        } else if i + 1 == n {
            CheckpointKind::End
        } else {
            CheckpointKind::Split
        };
        if cp.kind != expected {
            bail!(
                "checkpoint[{i}] kind is {:?}, expected {expected:?} (index 0 = Start, last = \
                 End, middle = Split)",
                cp.kind
            );
        }
        ensure!(
            !cp.label.trim().is_empty(),
            "checkpoint[{i}] label is empty (encoder requires non-empty labels)"
        );
    }

    let mut lines = Vec::with_capacity(1 + n);

    let title = format!("LS title {}", track.name);
    let title_cp = title.chars().count();
    ensure!(
        title_cp <= MAX_LINE_CP,
        "title line is {title_cp} cp; cap is {MAX_LINE_CP}"
    );
    lines.push(title);

    for (i, cp) in track.checkpoints.iter().enumerate() {
        match &cp.trigger {
            Trigger::Aabb(aabb) => {
                let kw = match cp.kind {
                    CheckpointKind::Start => "start",
                    CheckpointKind::Split => "cp",
                    CheckpointKind::End => "end",
                };
                let (min, size) = aabb_to_min_size(*aabb)?;
                let coords = format!(
                    "{},{},{} {},{},{}",
                    min[0], min[1], min[2], size[0], size[1], size[2]
                );

                let inline = if cp.label.is_empty() {
                    format!("LS {kw} {coords}")
                } else {
                    format!("LS {kw} {coords} {}", cp.label)
                };
                if inline.chars().count() <= MAX_LINE_CP {
                    lines.push(inline);
                    continue;
                }

                let bare = format!("LS {kw} {coords}");
                let bare_cp = bare.chars().count();
                ensure!(
                    bare_cp <= MAX_LINE_CP,
                    "checkpoint[{i}] `{kw}` line without label is {bare_cp} cp; cap is \
                     {MAX_LINE_CP}"
                );
                lines.push(bare);

                let label_line = format!("LS label {}", cp.label);
                let label_cp = label_line.chars().count();
                ensure!(
                    label_cp <= MAX_LINE_CP,
                    "checkpoint[{i}] label too long: standalone `LS label` line is {label_cp} cp; \
                     cap is {MAX_LINE_CP}"
                );
                lines.push(label_line);
            }
            Trigger::MapLoaded(name) => {
                ensure!(!name.trim().is_empty(), "checkpoint[{i}] map name is empty");
                let kw = match cp.kind {
                    CheckpointKind::Start | CheckpointKind::Split => "map",
                    CheckpointKind::End => "endmap",
                };
                let map_line = format!("LS {kw} {name}");
                let map_cp = map_line.chars().count();
                ensure!(
                    map_cp <= MAX_LINE_CP,
                    "checkpoint[{i}] `{kw}` line is {map_cp} cp; cap is {MAX_LINE_CP}"
                );
                lines.push(map_line);

                let label_line = format!("LS label {}", cp.label);
                let label_cp = label_line.chars().count();
                ensure!(
                    label_cp <= MAX_LINE_CP,
                    "checkpoint[{i}] label too long: standalone `LS label` line is {label_cp} cp; \
                     cap is {MAX_LINE_CP}"
                );
                lines.push(label_line);
            }
        }
    }

    for (i, line) in lines.iter().enumerate() {
        let cp_len = line.chars().count();
        debug_assert!(
            cp_len <= MAX_LINE_CP,
            "line[{i}] is {cp_len} cp, exceeds cap {MAX_LINE_CP}: {line}"
        );
    }

    Ok(lines)
}

#[cfg(test)]
mod tests {
    use classicube_sys::Vec3;

    use super::*;
    use crate::plugin::splits::geometry::{Aabb, Checkpoint, CheckpointKind};

    fn cp(
        kind: CheckpointKind,
        min: (f32, f32, f32),
        max: (f32, f32, f32),
        label: &str,
    ) -> Checkpoint {
        Checkpoint {
            kind,
            trigger: Trigger::Aabb(Aabb {
                min: Vec3::new(min.0, min.1, min.2),
                max: Vec3::new(max.0, max.1, max.2),
            }),
            label: label.into(),
        }
    }

    fn cp_map(kind: CheckpointKind, name: &str, label: &str) -> Checkpoint {
        Checkpoint {
            kind,
            trigger: Trigger::MapLoaded(name.into()),
            label: label.into(),
        }
    }

    fn loadtest_track() -> Track {
        Track {
            name: "loadtest".into(),
            checkpoints: vec![
                cp(
                    CheckpointKind::Start,
                    (0.0, 0.0, 0.0),
                    (2.0, 4.0, 2.0),
                    "start",
                ),
                cp(
                    CheckpointKind::Split,
                    (10.0, 0.0, 0.0),
                    (12.0, 4.0, 2.0),
                    "split 1",
                ),
                cp(
                    CheckpointKind::Split,
                    (20.0, 0.0, 0.0),
                    (22.0, 4.0, 2.0),
                    "split 2",
                ),
                cp(
                    CheckpointKind::End,
                    (30.0, 0.0, 0.0),
                    (32.0, 4.0, 2.0),
                    "end",
                ),
            ],
        }
    }

    fn assert_lines_within_cap(lines: &[String]) {
        for line in lines {
            let cp_len = line.chars().count();
            assert!(
                cp_len <= MAX_LINE_CP,
                "line `{line}` is {cp_len} cp (cap {MAX_LINE_CP})"
            );
        }
    }

    #[test]
    fn loadtest_round_trip_all_inline() {
        let track = loadtest_track();
        let lines = encode_for_chat(&track).unwrap();
        assert_eq!(lines.len(), 1 + 4);
        assert_lines_within_cap(&lines);
        assert!(lines[0].starts_with("LS title "));
        assert!(lines[1].starts_with("LS start "));
        assert!(lines[2].starts_with("LS cp "));
        assert!(lines[3].starts_with("LS cp "));
        assert!(lines[4].starts_with("LS end "));
    }

    #[test]
    fn two_checkpoint_round_trip_has_three_lines() {
        let track = Track {
            name: "T".into(),
            checkpoints: vec![
                cp(CheckpointKind::Start, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0), "s"),
                cp(CheckpointKind::End, (10.0, 0.0, 0.0), (12.0, 4.0, 2.0), "e"),
            ],
        };
        let lines = encode_for_chat(&track).unwrap();
        assert_eq!(lines.len(), 3);
        assert_lines_within_cap(&lines);
    }

    #[test]
    fn rejects_single_checkpoint_track() {
        let track = Track {
            name: "T".into(),
            checkpoints: vec![cp(
                CheckpointKind::Start,
                (0.0, 0.0, 0.0),
                (2.0, 4.0, 2.0),
                "only",
            )],
        };
        assert!(encode_for_chat(&track).is_err());
    }

    #[test]
    fn rejects_empty_track_name() {
        let track = Track {
            name: "   ".into(),
            checkpoints: vec![
                cp(CheckpointKind::Start, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0), "s"),
                cp(CheckpointKind::End, (10.0, 0.0, 0.0), (12.0, 4.0, 2.0), "e"),
            ],
        };
        assert!(encode_for_chat(&track).is_err());
    }

    #[test]
    fn rejects_empty_checkpoint_label() {
        let track = Track {
            name: "T".into(),
            checkpoints: vec![
                cp(CheckpointKind::Start, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0), ""),
                cp(CheckpointKind::End, (10.0, 0.0, 0.0), (12.0, 4.0, 2.0), "e"),
            ],
        };
        assert!(encode_for_chat(&track).is_err());
    }

    #[test]
    fn rejects_overlong_track_name() {
        // "LS title " is 9 cp; pad to push past 61 cp.
        let track = Track {
            name: "x".repeat(60),
            checkpoints: vec![
                cp(CheckpointKind::Start, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0), "s"),
                cp(CheckpointKind::End, (10.0, 0.0, 0.0), (12.0, 4.0, 2.0), "e"),
            ],
        };
        assert!(encode_for_chat(&track).is_err());
    }

    #[test]
    fn rejects_label_too_long_even_standalone() {
        // "LS label " is 9 cp; >52 cp label overflows the standalone line.
        let label = "x".repeat(60);
        let track = Track {
            name: "T".into(),
            checkpoints: vec![
                cp(CheckpointKind::Start, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0), "s"),
                cp(
                    CheckpointKind::End,
                    (10.0, 0.0, 0.0),
                    (12.0, 4.0, 2.0),
                    &label,
                ),
            ],
        };
        assert!(encode_for_chat(&track).is_err());
    }

    #[test]
    fn falls_back_to_separate_label_line_when_inline_overflows() {
        // Inline `LS start 0,0,0 2,4,2 <label>` = 21 + label cp; cap 61
        // → label needs > 40 cp to overflow inline but ≤ 52 cp to fit
        // standalone. 45 cp lands in that range.
        let label = "x".repeat(45);
        let track = Track {
            name: "T".into(),
            checkpoints: vec![
                cp(
                    CheckpointKind::Start,
                    (0.0, 0.0, 0.0),
                    (2.0, 4.0, 2.0),
                    &label,
                ),
                cp(CheckpointKind::End, (10.0, 0.0, 0.0), (12.0, 4.0, 2.0), "e"),
            ],
        };
        let lines = encode_for_chat(&track).unwrap();
        assert_eq!(lines.len(), 1 + 2 + 1);
        assert!(lines[1].starts_with("LS start ") && !lines[1].ends_with(&label));
        assert_eq!(lines[2], format!("LS label {label}"));
        assert_lines_within_cap(&lines);
    }

    #[test]
    fn preserves_multi_space_label_inline() {
        let track = Track {
            name: "T".into(),
            checkpoints: vec![
                cp(
                    CheckpointKind::Start,
                    (0.0, 0.0, 0.0),
                    (2.0, 4.0, 2.0),
                    "my  multi  word  label",
                ),
                cp(
                    CheckpointKind::End,
                    (10.0, 0.0, 0.0),
                    (12.0, 4.0, 2.0),
                    "end",
                ),
            ],
        };
        let lines = encode_for_chat(&track).unwrap();
        assert!(
            lines[1].ends_with(" my  multi  word  label"),
            "got: {}",
            lines[1]
        );
    }

    #[test]
    fn rejects_aabb_extent_over_255() {
        let track = Track {
            name: "T".into(),
            checkpoints: vec![
                cp(
                    CheckpointKind::Start,
                    (0.0, 0.0, 0.0),
                    (300.0, 4.0, 2.0),
                    "s",
                ),
                cp(CheckpointKind::End, (10.0, 0.0, 0.0), (12.0, 4.0, 2.0), "e"),
            ],
        };
        assert!(encode_for_chat(&track).is_err());
    }

    #[test]
    fn rejects_split_at_index_zero() {
        let track = Track {
            name: "T".into(),
            checkpoints: vec![
                cp(CheckpointKind::Split, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0), "a"),
                cp(CheckpointKind::End, (10.0, 0.0, 0.0), (12.0, 4.0, 2.0), "e"),
            ],
        };
        assert!(encode_for_chat(&track).is_err());
    }

    #[test]
    fn rejects_start_at_middle_index() {
        let track = Track {
            name: "T".into(),
            checkpoints: vec![
                cp(CheckpointKind::Start, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0), "s"),
                cp(
                    CheckpointKind::Start,
                    (10.0, 0.0, 0.0),
                    (12.0, 4.0, 2.0),
                    "mid",
                ),
                cp(CheckpointKind::End, (20.0, 0.0, 0.0), (22.0, 4.0, 2.0), "e"),
            ],
        };
        assert!(encode_for_chat(&track).is_err());
    }

    #[test]
    fn rejects_end_at_non_last_index() {
        let track = Track {
            name: "T".into(),
            checkpoints: vec![
                cp(CheckpointKind::Start, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0), "s"),
                cp(
                    CheckpointKind::End,
                    (10.0, 0.0, 0.0),
                    (12.0, 4.0, 2.0),
                    "mid",
                ),
                cp(CheckpointKind::End, (20.0, 0.0, 0.0), (22.0, 4.0, 2.0), "e"),
            ],
        };
        assert!(encode_for_chat(&track).is_err());
    }

    #[test]
    fn map_only_two_checkpoint_emits_map_and_endmap() {
        let track = Track {
            name: "M".into(),
            checkpoints: vec![
                cp_map(CheckpointKind::Start, "spawn", "start"),
                cp_map(CheckpointKind::End, "goal", "end"),
            ],
        };
        let lines = encode_for_chat(&track).unwrap();
        assert_eq!(lines.len(), 1 + 2 + 2, "title + 2 cps × (kw + label)");
        assert_lines_within_cap(&lines);
        assert_eq!(lines[0], "LS title M");
        assert_eq!(lines[1], "LS map spawn");
        assert_eq!(lines[2], "LS label start");
        assert_eq!(lines[3], "LS endmap goal");
        assert_eq!(lines[4], "LS label end");
    }

    #[test]
    fn mixed_aabb_and_map_interleave() {
        let track = Track {
            name: "T".into(),
            checkpoints: vec![
                cp(CheckpointKind::Start, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0), "s"),
                cp_map(CheckpointKind::Split, "mid_map", "midmap"),
                cp(
                    CheckpointKind::Split,
                    (20.0, 0.0, 0.0),
                    (22.0, 4.0, 2.0),
                    "s2",
                ),
                cp_map(CheckpointKind::End, "goal", "fin"),
            ],
        };
        let lines = encode_for_chat(&track).unwrap();
        assert_lines_within_cap(&lines);
        assert!(lines[0].starts_with("LS title "));
        assert!(lines[1].starts_with("LS start "));
        assert_eq!(lines[2], "LS map mid_map");
        assert_eq!(lines[3], "LS label midmap");
        assert!(lines[4].starts_with("LS cp "));
        assert_eq!(lines[5], "LS endmap goal");
        assert_eq!(lines[6], "LS label fin");
    }

    #[test]
    fn rejects_empty_map_name() {
        let track = Track {
            name: "T".into(),
            checkpoints: vec![
                cp_map(CheckpointKind::Start, "  ", "start"),
                cp_map(CheckpointKind::End, "goal", "end"),
            ],
        };
        assert!(encode_for_chat(&track).is_err());
    }

    #[test]
    fn rejects_overlong_map_name() {
        // "LS map " is 7 cp; cap 61 → name > 54 cp overflows.
        let track = Track {
            name: "T".into(),
            checkpoints: vec![
                cp_map(CheckpointKind::Start, &"x".repeat(55), "s"),
                cp_map(CheckpointKind::End, "goal", "e"),
            ],
        };
        assert!(encode_for_chat(&track).is_err());
    }

    #[test]
    fn rejects_overlong_endmap_name() {
        // "LS endmap " is 10 cp; cap 61 → name > 51 cp overflows.
        let track = Track {
            name: "T".into(),
            checkpoints: vec![
                cp_map(CheckpointKind::Start, "spawn", "s"),
                cp_map(CheckpointKind::End, &"y".repeat(52), "e"),
            ],
        };
        assert!(encode_for_chat(&track).is_err());
    }
}
