use crate::{chat_codec, plugin::splits::geometry::Track};

/// Visible chat-line marker. Dark-magenta color code — a vanilla
/// player can't type a leading color code, but servers can, so a
/// legitimate server chat line can also start with `&m`. The marker
/// is therefore a *candidate* indicator only; the full decode cascade
/// is what actually distinguishes a track frame from noise.
pub const MARKER: &str = "&m";

/// Pure: try to interpret `text` as a chat-protocol-delivered track.
/// Returns `Some(track)` only when every step of the decode cascade
/// succeeds; `None` for any failure (missing marker, encoded blob
/// contains forbidden chars, zstd or postcard failure, size cap
/// exceeded). The caller treats `None` as "render this chat line
/// normally."
pub fn try_decode_chat_line(text: &str) -> Option<Track> {
    let encoded = text.strip_prefix(MARKER)?;
    let wire = chat_codec::decode(encoded).ok()?;
    Track::decode_from_wire(&wire).ok()
}

#[cfg(test)]
mod tests {
    use classicube_sys::Vec3;

    use super::*;
    use crate::plugin::{
        splits::geometry::{Aabb, Checkpoint, CheckpointKind},
        track_source::encode::encode_for_chat,
    };

    fn track_n(n: usize) -> Track {
        Track {
            name: String::new(),
            checkpoints: (0..n)
                .map(|i| Checkpoint {
                    kind: if i == 0 {
                        CheckpointKind::Start
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
    fn round_trip_minimal_track() {
        let track = track_n(1);
        let line = encode_for_chat(&track).unwrap();
        let decoded = try_decode_chat_line(&line).unwrap();
        assert_eq!(decoded, track);
    }

    #[test]
    fn decode_rejects_non_prefixed_line() {
        assert!(try_decode_chat_line("hello world").is_none());
    }

    #[test]
    fn decode_rejects_prefix_only() {
        assert!(try_decode_chat_line(MARKER).is_none());
    }

    #[test]
    fn decode_rejects_prefix_with_space() {
        // Space is in chat_codec's FORBIDDEN set; decode fails immediately.
        assert!(try_decode_chat_line("&m hello world").is_none());
    }

    #[test]
    fn decode_rejects_prefix_with_garbage_bytes() {
        // Valid chat_codec chars but not a zstd frame.
        assert!(try_decode_chat_line("&mABCD").is_none());
    }

    #[test]
    fn decode_rejects_zstd_bomb() {
        // Hand-crafted zstd frame declaring a huge frame_content_size.
        // Magic 0x28 0xB5 0x2F 0xFD, frame_header_descriptor with
        // single-segment + FCS field size = 3 (8 bytes), FCS = 1<<40
        // (1 TiB). The decoder must reject before allocating.
        let mut bomb = vec![0x28, 0xB5, 0x2F, 0xFD];
        // FHD: single_segment=1 (bit 5), fcs_field_size=3 (bits 6-7 = 11)
        bomb.push(0b1110_0000);
        // FCS: 8-byte little-endian = 1 << 40
        bomb.extend_from_slice(&(1u64 << 40).to_le_bytes());
        // Followed by an arbitrary RLE block claiming huge output.
        // We don't need a complete frame — decode_from_wire either
        // bails on the cap or fails parsing; either way returns Err.
        let encoded = format!("&m{}", chat_codec::encode(&bomb));
        assert!(try_decode_chat_line(&encoded).is_none());
    }
}
