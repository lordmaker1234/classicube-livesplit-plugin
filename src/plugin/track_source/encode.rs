use anyhow::{Result, ensure};

use crate::{
    chat_codec,
    plugin::{splits::geometry::Track, track_source::decode::MARKER},
};

/// Maximum total chat-line length, in CP437 codepoints. ClassiCube's
/// `INPUTWIDGET_LEN`/`STRING_SIZE` wrap point is 64; subtract 3 for the
/// default color prefix the server prepends on echo, leaving 61 cp for
/// our payload. Going over means `LineWrapper` re-splits the line and
/// inserts a `> &X` continuation marker, which we don't reassemble on
/// the receive side.
const MAX_LINE_CP: usize = 64 - 3;

/// Postcard + zstd + chat-codec a `Track` into a single chat line of
/// the form `&m<encoded>`. Errors if the result exceeds the
/// single-line cap — the caller should treat that as "track too big
/// for the chat-protocol path; use file-based delivery instead."
pub fn encode_for_chat(track: &Track) -> Result<String> {
    let wire = track.encode_to_wire()?;
    let encoded = chat_codec::encode(&wire);
    let line = format!("{MARKER}{encoded}");
    let cp_len = line.chars().count();
    ensure!(
        cp_len <= MAX_LINE_CP,
        "encoded track line is {cp_len} cp; single-line cap is {MAX_LINE_CP} cp (track too large \
         for chat-protocol delivery)"
    );
    Ok(line)
}

#[cfg(test)]
mod tests {
    use classicube_sys::Vec3;

    use super::*;
    use crate::plugin::splits::geometry::{Aabb, Checkpoint, CheckpointKind};

    fn track_n_checkpoints(n: usize) -> Track {
        Track {
            name: "T".into(),
            checkpoints: (0..n)
                .map(|i| Checkpoint {
                    kind: if i == 0 {
                        CheckpointKind::Start
                    } else if i + 1 == n {
                        CheckpointKind::End
                    } else {
                        CheckpointKind::Split
                    },
                    aabb: Aabb::new(
                        Vec3::new(i as f32, 0.0, 0.0),
                        Vec3::new(i as f32 + 1.0, 1.0, 1.0),
                    ),
                    label: None,
                })
                .collect(),
        }
    }

    #[test]
    fn encode_fits_within_cap_for_small_track() {
        let line = encode_for_chat(&track_n_checkpoints(1)).unwrap();
        let cp_len = line.chars().count();
        assert!(
            cp_len <= MAX_LINE_CP,
            "got {cp_len} cp, cap is {MAX_LINE_CP}"
        );
        assert!(line.starts_with(MARKER));
    }

    #[test]
    fn encode_rejects_oversized_track() {
        // Sweep upward until encode_for_chat rejects. Establishes that
        // *some* size is rejected; the precise threshold depends on
        // postcard + zstd ratios so we don't pin it.
        let mut last_ok = None;
        for n in 1..=64 {
            match encode_for_chat(&track_n_checkpoints(n)) {
                Ok(_) => last_ok = Some(n),
                Err(_) => {
                    assert!(
                        last_ok.is_some(),
                        "smallest track ({n}=1) should fit; saw immediate rejection"
                    );
                    return;
                }
            }
        }
        panic!("no checkpoint count up to 64 triggered the cap; cap is broken");
    }
}
