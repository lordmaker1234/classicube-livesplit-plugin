#[cfg(test)]
mod tests;

use anyhow::{Result, bail, ensure};

use crate::plugin::splits::geometry::{
    CheckpointKind, Track, Trigger, aabb_to_min_size, validate_pause_resume_pairing,
};

/// Maximum per-line length, in codepoints. ClassiCube's
/// `INPUTWIDGET_LEN`/`STRING_SIZE` wrap point is 64; subtract 3 for the
/// default color prefix the server prepends on echo, leaving 61 cp for
/// our payload. Going over means `LineWrapper` re-splits the line and
/// inserts a `> &X` continuation marker, which we don't reassemble on
/// the receive side.
pub(crate) const MAX_LINE_CP: usize = 64 - 3;

/// Current on-wire `LS …` grammar version, emitted as the leading
/// `LS v<n>` line on both transports (chat broadcast + `.lss` disk
/// payload). Bump on any breaking grammar change (new keyword, coord
/// encoding, field order). Readers reject any other value with an
/// "unsupported version" diagnostic instead of risking a silent misparse.
pub(crate) const LS_FORMAT_VERSION: u32 = 1;

/// Encode a `Track` into a series of `LS …` chat lines. The caller is
/// responsible for emitting them — one per `/mb sign` block, or chained
/// into a single block via the `mb` arm of the command module.
///
/// Layout:
///   line[0]    = `LS v<n>` (format version; the receiver's reset anchor)
///   line[1]    = `LS title <name>` (chat-only; the disk variant skips it
///                -- the title lives in the `.lss` `<CategoryName>` -- so
///                on disk the version line is followed directly by the
///                first checkpoint)
///   line[2..n] = per checkpoint, in order. Each checkpoint emits one
///                of four keyword lines:
///                  `LS cp <min> <size> [label]`      (Split, AABB)
///                  `LS map <name> [label]`           (Split, MapLoaded)
///                  `LS pause <min> <size> [label]`   (Pause, AABB)
///                  `LS unpause <min> <size> [label]` (Resume, AABB)
///                Start uses `LS cp` / `LS map` like a Split (position
///                implies kind); End is the last checkpoint and uses
///                whichever wire form (cp / map) its trigger demands.
///                Labels are inline when they fit, otherwise the kind
///                line is emitted bare and followed by
///                `LS label <text>`. Map names cannot contain a space
///                (the first space delimits name from label); the
///                encoder errors if the runtime `MapLoaded(name)`
///                contains one. Pause/Resume kinds are AABB-only; the
///                encoder rejects `Trigger::MapLoaded` paired with
///                them.
///   line[-1]   = `LS end` terminator. The receiver promotes the
///                last buffered checkpoint's kind from `Split` to
///                `End` on this line (Pause/Resume can't be the last
///                checkpoint; the encoder enforces this above).
pub fn encode_for_chat(track: &Track) -> Result<Vec<String>> {
    encode_lines(track, true)
}

/// Encode a `Track`'s geometry into bare `LS …` lines for on-disk
/// storage (the `.lss` `ClassiCubeTrack` custom variable). Identical to
/// [`encode_for_chat`] except it never emits the title or labels: the
/// `LS title` line is dropped (the title lives in the `.lss`
/// `<CategoryName>`) and every checkpoint line is bare (`LS cp <min>
/// <size>` / `LS map <name>` / `LS pause …` / `LS unpause …`) with no
/// inline labels and no `LS label` follow-up lines. Labels live in the
/// `.lss` `<Segment>` elements instead, so a label-only edit yields
/// identical disk text (the writer's dedup gate then skips it). The
/// non-empty-name and non-empty-label invariants are also skipped --
/// name/label content is irrelevant to disk geometry (the re-canonicalize
/// path decodes to a name-less `Track`).
pub fn encode_for_disk(track: &Track) -> Result<Vec<String>> {
    encode_lines(track, false)
}

/// Shared body of [`encode_for_chat`] / [`encode_for_disk`]. When
/// `for_chat` is true, the chat-only metadata is emitted: the `LS title`
/// line, each checkpoint's label (inline, or in a follow-up `LS label`
/// line when the inline form overflows `MAX_LINE_CP`), and the
/// non-empty-name / non-empty-label checks. When false (disk), the title
/// line and all labels are omitted and those checks are skipped.
fn encode_lines(track: &Track, for_chat: bool) -> Result<Vec<String>> {
    let n = track.checkpoints.len();
    ensure!(
        n >= 2,
        "track has {n} checkpoint(s); need at least 2 (Start + End)"
    );
    if for_chat {
        ensure!(!track.name.trim().is_empty(), "track name is empty");
    }

    for (i, cp) in track.checkpoints.iter().enumerate() {
        let valid = if i == 0 {
            cp.kind == CheckpointKind::Start
        } else if i + 1 == n {
            cp.kind == CheckpointKind::End
        } else {
            matches!(
                cp.kind,
                CheckpointKind::Split | CheckpointKind::Pause | CheckpointKind::Resume
            )
        };
        if !valid {
            bail!(
                "checkpoint[{i}] kind is {:?}; expected Start at index 0, End at last index, and \
                 Split/Pause/Resume in between",
                cp.kind
            );
        }
        if for_chat {
            ensure!(
                !cp.label.trim().is_empty(),
                "checkpoint[{i}] label is empty (encoder requires non-empty labels)"
            );
        }
        if matches!(cp.kind, CheckpointKind::Pause | CheckpointKind::Resume)
            && !matches!(cp.trigger, Trigger::Aabb(_))
        {
            bail!(
                "checkpoint[{i}] is {:?} kind but trigger is not AABB; Pause/Resume kinds are \
                 AABB-only",
                cp.kind
            );
        }
    }

    validate_pause_resume_pairing(track)?;

    let mut lines = Vec::with_capacity(3 + n);

    // Leading version line, ahead of `LS title`. On the chat transport
    // this is the universal reset/re-sync anchor; on disk it gates the
    // batch decoder. Always emitted; the cap-check loop below covers it.
    lines.push(format!("LS v{LS_FORMAT_VERSION}"));

    // The title is chat-only metadata: it carries the sender's chosen
    // category name to the receiver. On disk the title comes from the
    // `.lss` `<CategoryName>`, so the disk variant skips this line
    // entirely (the same way it skips labels).
    if for_chat {
        let title = format!("LS title {}", track.name);
        let title_cp = title.chars().count();
        ensure!(
            title_cp <= MAX_LINE_CP,
            "title line is {title_cp} cp; cap is {MAX_LINE_CP}"
        );
        lines.push(title);
    }

    for (i, cp) in track.checkpoints.iter().enumerate() {
        let keyword = match cp.kind {
            CheckpointKind::Pause => "pause",
            CheckpointKind::Resume => "unpause",
            CheckpointKind::Start | CheckpointKind::Split | CheckpointKind::End => {
                match &cp.trigger {
                    Trigger::Aabb(_) => "cp",
                    Trigger::MapLoaded(_) => "map",
                }
            }
        };

        let body = match &cp.trigger {
            Trigger::Aabb(aabb) => {
                let (min, size) = aabb_to_min_size(*aabb)?;
                format!(
                    "{},{},{} {},{},{}",
                    min[0], min[1], min[2], size[0], size[1], size[2]
                )
            }
            Trigger::MapLoaded(name) => {
                ensure!(!name.trim().is_empty(), "checkpoint[{i}] map name is empty");
                ensure!(
                    !name.contains(' '),
                    "checkpoint[{i}] map name `{name}` contains a space; map names on the wire \
                     cannot contain spaces (the space delimits name from label)"
                );
                name.clone()
            }
        };

        emit_keyword_line(&mut lines, i, keyword, &body, &cp.label, for_chat)?;
    }

    lines.push("LS end".to_string());

    for (i, line) in lines.iter().enumerate() {
        let cp_len = line.chars().count();
        debug_assert!(
            cp_len <= MAX_LINE_CP,
            "line[{i}] is {cp_len} cp, exceeds cap {MAX_LINE_CP}: {line}"
        );
    }

    Ok(lines)
}

/// Push the `LS <keyword> <body> [label]` line(s) for one checkpoint.
/// `body` is the already-formatted coord-triple pair (`<min> <size>`) or
/// map name. With `for_chat`, the label rides inline when it fits the
/// per-line cap, otherwise the line is emitted bare followed by an
/// `LS label <text>` line. Without `for_chat`, only the bare line is
/// emitted (disk geometry: labels come from `<Segment>` elements).
fn emit_keyword_line(
    lines: &mut Vec<String>,
    i: usize,
    keyword: &str,
    body: &str,
    label: &str,
    for_chat: bool,
) -> Result<()> {
    if for_chat {
        let inline = format!("LS {keyword} {body} {label}");
        if inline.chars().count() <= MAX_LINE_CP {
            lines.push(inline);
            return Ok(());
        }
    }

    let bare = format!("LS {keyword} {body}");
    let bare_cp = bare.chars().count();
    ensure!(
        bare_cp <= MAX_LINE_CP,
        "checkpoint[{i}] `{keyword}` line without label is {bare_cp} cp; cap is {MAX_LINE_CP}"
    );
    lines.push(bare);

    if for_chat {
        let label_line = format!("LS label {label}");
        let label_cp = label_line.chars().count();
        ensure!(
            label_cp <= MAX_LINE_CP,
            "checkpoint[{i}] label too long: standalone `LS label` line is {label_cp} cp; cap is \
             {MAX_LINE_CP}"
        );
        lines.push(label_line);
    }

    Ok(())
}
