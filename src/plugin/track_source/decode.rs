#[cfg(test)]
mod tests;

use std::{cell::RefCell, mem};

use anyhow::{Result, anyhow, bail, ensure};

use crate::plugin::{
    splits::geometry::{
        Aabb, Checkpoint, CheckpointKind, Track, Trigger, aabb_from_min_size,
        validate_pause_resume_pairing,
    },
    track_source::encode::LS_FORMAT_VERSION,
};

/// Result of feeding a single chat line to the receiver.
pub enum FrameOutcome {
    /// Line is not one of ours (no `LS ` prefix). Caller should let it
    /// render normally.
    NotOurs,
    /// Line looked like ours but didn't parse / didn't match the current
    /// state. Caller chat-prints the diagnostic and falls through so the
    /// raw line still renders.
    ParseError(String),
    /// Line is part of a track in progress. Caller suppresses it.
    Buffered,
    /// Final line of a track. Caller loads the track and suppresses.
    Loaded(Track),
}

#[derive(Debug)]
enum State {
    Idle,
    /// `LS v<n>` accepted (version matched); next valid line is
    /// `LS title <name>`.
    NeedTitle,
    /// `LS title` accepted; next valid line is `cp` or `map` (which
    /// becomes the Start-kind checkpoint).
    NeedFirst {
        name: String,
    },
    /// At least one checkpoint is in `slots`; the most recent one
    /// already has its label populated. Next line is `cp` / `map` /
    /// `pause` / `unpause` / `end` / `title`.
    NeedNext {
        name: String,
        slots: Vec<Checkpoint>,
    },
    /// The most recent checkpoint push has an empty label. Next line
    /// must be `LS label <text>` (or `LS title <name>` to reset).
    NeedLabel {
        name: String,
        slots: Vec<Checkpoint>,
    },
}

thread_local! {
    static STATE: RefCell<State> = const { RefCell::new(State::Idle) };
}

/// Strip one leading `&X` color code (where X is ASCII alphanumeric —
/// covers stock `&0`-`&f` and CPE custom codes like `&S`). The encoder's
/// `MAX_LINE_CP` budget already anticipates a server-prepended color
/// prefix on echo (observed on ClassiCube official servers: lines come
/// through as `&7LS title …`); without this strip the receiver would
/// `NotOurs` every frame the chained `mb` form produces. MCGalaxy
/// collapses runs of color codes to a single code before broadcast, so
/// at most one `&X` prefix ever reaches us.
fn strip_leading_color_code(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && bytes[0] == b'&' && bytes[1].is_ascii_alphanumeric() {
        &s[2..]
    } else {
        s
    }
}

/// Classify a single line into a parsed [`Line`], or `None` when it
/// isn't one of ours (no `LS ` prefix after an optional leading color
/// code). The keyword/coord parsing here is shared by the streaming chat
/// path ([`feed_chat_line`]) and the batch disk decoder
/// ([`decode_geometry`]); only the surrounding state handling differs.
fn classify_line(text: &str) -> Option<Result<Line, String>> {
    let text = strip_leading_color_code(text);
    let after_prefix = text.strip_prefix("LS ")?;

    let (keyword, rest) = match after_prefix.find(' ') {
        Some(i) => (&after_prefix[..i], &after_prefix[i + 1..]),
        None => (after_prefix, ""),
    };

    let parsed = match keyword {
        "title" => parse_title(rest),
        "label" => parse_label(rest),
        "cp" => parse_aabb_line(rest).map(|(aabb, label)| Line::Aabb {
            kind: CheckpointKind::Split,
            aabb,
            label,
        }),
        "map" => parse_map(rest),
        "pause" => parse_aabb_line(rest).map(|(aabb, label)| Line::Aabb {
            kind: CheckpointKind::Pause,
            aabb,
            label,
        }),
        "unpause" => parse_aabb_line(rest).map(|(aabb, label)| Line::Aabb {
            kind: CheckpointKind::Resume,
            aabb,
            label,
        }),
        "end" => parse_end(rest),
        // The version keyword is the token `v<digits>` (`v1`, `v2`, …);
        // the number rides in the keyword itself, not `rest`.
        other => match other.strip_prefix('v').and_then(|d| d.parse::<u32>().ok()) {
            Some(version) if rest.is_empty() => Ok(Line::Version { version }),
            Some(_) => Err(format!("`LS {other}` takes no arguments, got `{rest}`")),
            None => Err(format!("unknown keyword `{other}`")),
        },
    };

    Some(parsed)
}

/// Feed one chat line. Updates the thread-local state machine and
/// returns the outcome the caller should react to.
pub fn feed_chat_line(text: &str) -> FrameOutcome {
    let line = match classify_line(text) {
        None => return FrameOutcome::NotOurs,
        Some(Ok(l)) => l,
        Some(Err(e)) => return FrameOutcome::ParseError(e),
    };

    STATE.with(|cell| {
        let mut state = cell.borrow_mut();
        transition(&mut state, line)
    })
}

/// Batch-decode geometry-only `LS …` text (the `.lss` `ClassiCubeTrack`
/// custom variable) into a label-less `Track`. Unlike [`feed_chat_line`]
/// this is pure -- no thread-local state -- because the disk read runs
/// off the main thread. Labels are intentionally dropped: they live in
/// the `.lss` `<Segment>` elements and the storage layer re-attaches
/// them.
///
/// Tolerant of hand-editing: a trailing `\r` (CRLF) and leading
/// whitespace (formatter indentation) are stripped per line and blank
/// lines are skipped. An inline label on a checkpoint line is ignored;
/// a standalone `LS label` line is a parse error (labels don't belong in
/// the geometry payload). Requires a leading `LS v<n>` version line
/// matching this build's [`LS_FORMAT_VERSION`] (a missing or mismatched
/// version is rejected -- pre-version files fail here and regenerate on
/// the next save), then `LS title`, `LS end`, and at least 2
/// checkpoints, then runs [`validate_pause_resume_pairing`]. Position
/// implies kind: the first checkpoint becomes `Start`, `LS end` promotes
/// the last `Split` to `End` (Pause/Resume can't be last), and a leading
/// `LS pause` / `LS unpause` is rejected.
pub fn decode_geometry(text: &str) -> Result<Track> {
    let mut version_seen = false;
    let mut name: Option<String> = None;
    let mut checkpoints: Vec<Checkpoint> = Vec::new();
    let mut ended = false;

    for raw in text.lines() {
        let line = raw.trim_start().trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let Some(parsed) = classify_line(line) else {
            bail!("not an `LS` line: `{line}`");
        };
        let parsed = parsed.map_err(|e| anyhow!("{e}"))?;

        ensure!(!ended, "content after `LS end`");

        match parsed {
            Line::Version { version } => {
                ensure!(!version_seen, "duplicate `LS v<n>` version line");
                ensure!(
                    name.is_none() && checkpoints.is_empty(),
                    "`LS v<n>` version line must come first"
                );
                ensure!(
                    version == LS_FORMAT_VERSION,
                    "unsupported LS format version {version}; this build speaks \
                     v{LS_FORMAT_VERSION}"
                );
                version_seen = true;
            }
            Line::Title { name: t } => {
                ensure!(
                    version_seen,
                    "missing `LS v<n>` version line before `LS title`"
                );
                ensure!(name.is_none(), "duplicate `LS title` line");
                ensure!(checkpoints.is_empty(), "`LS title` after a checkpoint");
                name = Some(t);
            }
            Line::Aabb { kind, aabb, .. } => {
                ensure!(name.is_some(), "checkpoint before `LS title`");
                let kind = if checkpoints.is_empty() {
                    // First checkpoint is always Start; a leading
                    // `LS pause` / `LS unpause` would land here with a
                    // Pause/Resume kind -- reject it.
                    ensure!(
                        kind == CheckpointKind::Split,
                        "first checkpoint must be `LS cp` or `LS map` (Start kind), not `LS \
                         pause` / `LS unpause`"
                    );
                    CheckpointKind::Start
                } else {
                    kind
                };
                checkpoints.push(Checkpoint {
                    kind,
                    trigger: Trigger::Aabb(aabb),
                    label: String::new(),
                });
            }
            Line::Map { name: map, .. } => {
                ensure!(name.is_some(), "checkpoint before `LS title`");
                let kind = if checkpoints.is_empty() {
                    CheckpointKind::Start
                } else {
                    CheckpointKind::Split
                };
                checkpoints.push(Checkpoint {
                    kind,
                    trigger: Trigger::MapLoaded(map),
                    label: String::new(),
                });
            }
            Line::End => {
                ensure!(name.is_some(), "`LS end` before `LS title`");
                ensure!(
                    checkpoints.len() >= 2,
                    "track needs at least 2 checkpoints before `LS end`"
                );
                let last = checkpoints.last_mut().expect("len >= 2 above");
                ensure!(
                    last.kind == CheckpointKind::Split,
                    "last checkpoint before `LS end` must be a plain checkpoint (`LS cp` / `LS \
                     map`), not {:?}",
                    last.kind
                );
                last.kind = CheckpointKind::End;
                ended = true;
            }
            Line::Label { .. } => {
                bail!(
                    "unexpected `LS label` line in geometry payload (labels live in <Segment> \
                     elements)"
                );
            }
        }
    }

    let name = name.ok_or_else(|| anyhow!("missing `LS title` line"))?;
    ensure!(ended, "missing `LS end` line");

    let track = Track { name, checkpoints };
    validate_pause_resume_pairing(&track)?;
    Ok(track)
}

enum Line {
    /// `LS v<n>` — the leading format-version line. The reset anchor on
    /// the chat transport; the first required line on disk.
    Version {
        version: u32,
    },
    Title {
        name: String,
    },
    /// `LS cp` (Split kind), `LS pause` (Pause kind), or `LS unpause`
    /// (Resume kind). Carries the kind the line keyword implies; the
    /// state machine downgrades to `Start` for the first checkpoint
    /// and promotes to `End` on `LS end`.
    Aabb {
        kind: CheckpointKind,
        aabb: Aabb,
        label: Option<String>,
    },
    Map {
        name: String,
        label: Option<String>,
    },
    End,
    Label {
        text: String,
    },
}

fn parse_title(rest: &str) -> Result<Line, String> {
    if rest.trim().is_empty() {
        return Err("title name is empty".to_string());
    }
    Ok(Line::Title {
        name: rest.to_string(),
    })
}

fn parse_map(rest: &str) -> Result<Line, String> {
    if rest.trim().is_empty() {
        return Err("map name is empty".to_string());
    }
    let mut parts = rest.splitn(2, ' ');
    let name = parts.next().expect("splitn yields >= 1 part").to_string();
    if name.is_empty() {
        return Err("map name is empty".to_string());
    }
    let label = match parts.next() {
        Some(s) => {
            if s.trim().is_empty() {
                return Err("inline label is empty".to_string());
            }
            Some(s.to_string())
        }
        None => None,
    };
    Ok(Line::Map { name, label })
}

fn parse_label(rest: &str) -> Result<Line, String> {
    if rest.trim().is_empty() {
        return Err("label text is empty".to_string());
    }
    Ok(Line::Label {
        text: rest.to_string(),
    })
}

fn parse_end(rest: &str) -> Result<Line, String> {
    if !rest.is_empty() {
        return Err(format!("`LS end` takes no arguments, got `{rest}`"));
    }
    Ok(Line::End)
}

/// Parse the body of an `LS cp` / `LS pause` / `LS unpause` line —
/// two coord triples plus an optional trailing label.
fn parse_aabb_line(rest: &str) -> Result<(Aabb, Option<String>), String> {
    let mut parts = rest.splitn(3, ' ');
    let min_str = parts
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "missing min triple".to_string())?;
    let size_str = parts
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "missing size triple".to_string())?;
    let label_str = parts.next();

    let min = parse_u16_triple(min_str)?;
    let size = parse_u8_triple(size_str)?;
    let aabb = aabb_from_min_size(min, size);

    let label = match label_str {
        Some(s) => {
            if s.trim().is_empty() {
                return Err("inline label is empty".to_string());
            }
            Some(s.to_string())
        }
        None => None,
    };

    Ok((aabb, label))
}

fn parse_u16_triple(s: &str) -> Result<[u16; 3], String> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 3 {
        return Err(format!(
            "expected 3 comma-separated min values, got {}",
            parts.len()
        ));
    }
    let mut out = [0u16; 3];
    for (i, p) in parts.iter().enumerate() {
        out[i] = p
            .parse::<u16>()
            .map_err(|e| format!("min[{i}] `{p}`: {e}"))?;
    }
    Ok(out)
}

fn parse_u8_triple(s: &str) -> Result<[u8; 3], String> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 3 {
        return Err(format!(
            "expected 3 comma-separated size values, got {}",
            parts.len()
        ));
    }
    let mut out = [0u8; 3];
    for (i, p) in parts.iter().enumerate() {
        out[i] = p
            .parse::<u8>()
            .map_err(|e| format!("size[{i}] `{p}`: {e}"))?;
    }
    Ok(out)
}

fn transition(state: &mut State, line: Line) -> FrameOutcome {
    // `LS v<n>` is the universal reset / re-sync anchor from any state:
    // every broadcast leads with it, so receiving one mid-parse cleanly
    // restarts the track. A mismatched version drops to `Idle` with a
    // clear diagnostic instead of risking a silent misparse.
    if let Line::Version { version } = line {
        if version != LS_FORMAT_VERSION {
            *state = State::Idle;
            return FrameOutcome::ParseError(format!(
                "unsupported LS format version {version}; this build speaks v{LS_FORMAT_VERSION}"
            ));
        }
        *state = State::NeedTitle;
        return FrameOutcome::Buffered;
    }

    let taken = mem::replace(state, State::Idle);

    match (taken, line) {
        // `LS title` is valid only immediately after the version line.
        (State::NeedTitle, Line::Title { name }) => {
            *state = State::NeedFirst { name };
            FrameOutcome::Buffered
        }

        // First checkpoint is always Start, regardless of the line's
        // declared kind. Pause/Resume kinds on the first line are
        // refused (the kind reported on the line came from the keyword,
        // so a "LS pause ..." as the first checkpoint would land here
        // with kind = Pause; reject it).
        (
            State::NeedFirst { name },
            Line::Aabb {
                kind: CheckpointKind::Pause | CheckpointKind::Resume,
                ..
            },
        ) => {
            *state = State::NeedFirst { name };
            FrameOutcome::ParseError(
                "first checkpoint must be `LS cp` or `LS map` (Start kind), not `LS pause` / `LS \
                 unpause`"
                    .to_string(),
            )
        }
        (
            State::NeedFirst { name },
            Line::Aabb {
                kind: _,
                aabb,
                label,
            },
        ) => push_first(state, name, Trigger::Aabb(aabb), label),
        (State::NeedFirst { name }, Line::Map { name: map, label }) => {
            push_first(state, name, Trigger::MapLoaded(map), label)
        }
        (State::NeedNext { name, slots }, Line::Aabb { kind, aabb, label }) => {
            push_next(state, name, slots, kind, Trigger::Aabb(aabb), label)
        }
        (State::NeedNext { name, slots }, Line::Map { name: map, label }) => push_next(
            state,
            name,
            slots,
            CheckpointKind::Split,
            Trigger::MapLoaded(map),
            label,
        ),
        (State::NeedNext { name, mut slots }, Line::End) => {
            if slots.len() < 2 {
                *state = State::NeedNext { name, slots };
                return FrameOutcome::ParseError(
                    "track needs at least 2 checkpoints before `LS end`".to_string(),
                );
            }
            // End promotes the previous Split (cp / map) to End. Pause
            // / Resume kinds can't be the last checkpoint — they'd
            // otherwise lose their pause-counter side effect to the
            // End promotion. Reject so the author's track stays well-
            // formed.
            let last_kind = slots.last().expect("len >= 2 above").kind;
            if !matches!(last_kind, CheckpointKind::Split) {
                *state = State::NeedNext { name, slots };
                return FrameOutcome::ParseError(format!(
                    "last checkpoint before `LS end` must be a plain checkpoint (`LS cp` / `LS \
                     map`), not {last_kind:?}"
                ));
            }
            slots.last_mut().expect("len >= 2 above").kind = CheckpointKind::End;
            let track = Track {
                name,
                checkpoints: slots,
            };
            // Belt-and-braces: the decoder can construct a `Track` that
            // `encode_for_chat` would have rejected (e.g. operator
            // hand-writing `LS …` frames from a sign), so re-validate
            // before adopting. Drop to `Idle` on failure — the track is
            // structurally broken; a fresh `LS title …` is the
            // recovery path.
            if let Err(e) = validate_pause_resume_pairing(&track) {
                *state = State::Idle;
                return FrameOutcome::ParseError(e.to_string());
            }
            FrameOutcome::Loaded(track)
        }
        (State::NeedLabel { name, mut slots }, Line::Label { text }) => {
            let last = slots
                .last_mut()
                .expect("NeedLabel always has at least one slot");
            last.label = text;
            *state = State::NeedNext { name, slots };
            FrameOutcome::Buffered
        }

        // --- ParseError fall-throughs ---
        // Nothing is valid before the leading version line.
        (
            taken @ State::Idle,
            Line::Title { .. }
            | Line::Aabb { .. }
            | Line::Map { .. }
            | Line::End
            | Line::Label { .. },
        ) => {
            *state = taken;
            FrameOutcome::ParseError("no `LS v<n>` version line yet".to_string())
        }
        // After the version line, only `LS title` is valid.
        (
            taken @ State::NeedTitle,
            Line::Aabb { .. } | Line::Map { .. } | Line::End | Line::Label { .. },
        ) => {
            *state = taken;
            FrameOutcome::ParseError("expected `LS title` after `LS v<n>`".to_string())
        }
        // A second `LS title` mid-track is no longer a reset (the version
        // line is); reject it so a stray title can't silently truncate.
        (
            taken @ (State::NeedFirst { .. } | State::NeedNext { .. } | State::NeedLabel { .. }),
            Line::Title { .. },
        ) => {
            *state = taken;
            FrameOutcome::ParseError(
                "unexpected `LS title`; a new track must start with `LS v<n>`".to_string(),
            )
        }
        (taken @ State::NeedFirst { .. }, Line::End) => {
            *state = taken;
            FrameOutcome::ParseError("no checkpoints before `LS end`".to_string())
        }
        (taken @ State::NeedFirst { .. }, Line::Label { .. }) => {
            *state = taken;
            FrameOutcome::ParseError("no checkpoint to label".to_string())
        }
        (taken @ State::NeedLabel { .. }, Line::Aabb { .. } | Line::Map { .. } | Line::End) => {
            *state = taken;
            FrameOutcome::ParseError("previous checkpoint not yet labeled".to_string())
        }
        (taken @ State::NeedNext { .. }, Line::Label { .. }) => {
            *state = taken;
            FrameOutcome::ParseError("checkpoint already has a label".to_string())
        }

        // Version was handled above.
        (_, Line::Version { .. }) => unreachable!("version handled above"),
    }
}

/// Build the first checkpoint (Start) from a NeedFirst transition.
fn push_first(
    state: &mut State,
    name: String,
    trigger: Trigger,
    label: Option<String>,
) -> FrameOutcome {
    match label {
        Some(label) => {
            *state = State::NeedNext {
                name,
                slots: vec![Checkpoint {
                    kind: CheckpointKind::Start,
                    trigger,
                    label,
                }],
            };
        }
        None => {
            *state = State::NeedLabel {
                name,
                slots: vec![Checkpoint {
                    kind: CheckpointKind::Start,
                    trigger,
                    label: String::new(),
                }],
            };
        }
    }
    FrameOutcome::Buffered
}

/// Build a subsequent checkpoint (Split / Pause / Resume) from a
/// NeedNext transition. The `kind` argument is the kind the wire
/// keyword implied — `LS cp` / `LS map` -> Split, `LS pause` ->
/// Pause, `LS unpause` -> Resume. `LS end` later promotes the last
/// Split to End.
fn push_next(
    state: &mut State,
    name: String,
    mut slots: Vec<Checkpoint>,
    kind: CheckpointKind,
    trigger: Trigger,
    label: Option<String>,
) -> FrameOutcome {
    match label {
        Some(label) => {
            slots.push(Checkpoint {
                kind,
                trigger,
                label,
            });
            *state = State::NeedNext { name, slots };
        }
        None => {
            slots.push(Checkpoint {
                kind,
                trigger,
                label: String::new(),
            });
            *state = State::NeedLabel { name, slots };
        }
    }
    FrameOutcome::Buffered
}
