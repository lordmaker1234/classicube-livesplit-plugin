use std::io::Read;

use anyhow::{Result, ensure};
use classicube_sys::Vec3;
use serde::{Deserialize, Deserializer, Serialize, Serializer, ser::Error as _};

use crate::plugin::livesplit::Command;

/// Upper bound on the postcard-decoded size of a wire-encoded `Track`.
/// Defends against zstd-bomb payloads from hostile servers: a 4-byte
/// crafted zstd frame can declare an arbitrarily large output, so the
/// decoder runs in streaming mode and bails the moment the cap is
/// exceeded. 16 KiB comfortably fits any plausible `Track` (postcard
/// adds ~26 bytes per checkpoint plus name/label varints).
const MAX_DECOMPRESSED_BYTES: usize = 16 * 1024;

/// Runtime keeps `min`/`max` as `Vec3<f32>`, but the chat wire form
/// is the manual [`Serialize`] / [`Deserialize`] impl below — `min`
/// quantized to `[u16; 3]` (block precision) and `size = max - min`
/// as `[u8; 3]`. ClassiCube world coords are non-negative and a
/// "big" CC map is roughly 700 × 300 × 1000 blocks, an order of
/// magnitude under `u16::MAX`, so `u16` covers any `min` with
/// margin; checkpoint volumes are typically a handful of blocks per
/// axis, so `u8` covers any `size` without ever triggering postcard's
/// 2-byte varint for the alternative `max: u16` form. Out-of-range
/// `min` values clamp; serialization errors if any axis extent
/// exceeds 255 blocks.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "value is clamp()'d to the u16 range before the cast"
)]
fn quantize_axis(v: f32) -> u16 {
    v.round().clamp(0.0, f32::from(u16::MAX)) as u16
}

fn vec3_to_u16(v: Vec3) -> [u16; 3] {
    [quantize_axis(v.x), quantize_axis(v.y), quantize_axis(v.z)]
}

fn u16_to_vec3([x, y, z]: [u16; 3]) -> Vec3 {
    Vec3::new(f32::from(x), f32::from(y), f32::from(z))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Serialize for Aabb {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let min = vec3_to_u16(self.min);
        let max = vec3_to_u16(self.max);
        let to_u8 = |extent: u16| {
            u8::try_from(extent).map_err(|_| {
                S::Error::custom(format!(
                    "AABB extent {extent} exceeds 255 blocks on one axis (chat wire-form cap)"
                ))
            })
        };
        let size = [
            to_u8(max[0].saturating_sub(min[0]))?,
            to_u8(max[1].saturating_sub(min[1]))?,
            to_u8(max[2].saturating_sub(min[2]))?,
        ];
        (min, size).serialize(s)
    }
}

impl<'de> Deserialize<'de> for Aabb {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let (min, size): ([u16; 3], [u8; 3]) = Deserialize::deserialize(d)?;
        let max = [
            min[0].saturating_add(u16::from(size[0])),
            min[1].saturating_add(u16::from(size[1])),
            min[2].saturating_add(u16::from(size[2])),
        ];
        Ok(Aabb {
            min: u16_to_vec3(min),
            max: u16_to_vec3(max),
        })
    }
}

impl Aabb {
    #[must_use]
    pub fn new(a: Vec3, b: Vec3) -> Self {
        Self {
            min: Vec3::new(a.x.min(b.x), a.y.min(b.y), a.z.min(b.z)),
            max: Vec3::new(a.x.max(b.x), a.y.max(b.y), a.z.max(b.z)),
        }
    }

    /// Half-open containment: `min <= p < max` per axis. Adjacent boxes that
    /// share a face will not both fire when the player straddles them.
    #[must_use]
    pub fn contains(&self, p: Vec3) -> bool {
        p.x >= self.min.x
            && p.x < self.max.x
            && p.y >= self.min.y
            && p.y < self.max.y
            && p.z >= self.min.z
            && p.z < self.max.z
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckpointKind {
    Start,
    Split,
    End,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Checkpoint {
    pub kind: CheckpointKind,
    pub aabb: Aabb,
    pub label: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub name: String,
    pub checkpoints: Vec<Checkpoint>,
}

/// Wire-only twin of [`Checkpoint`] used by `Track`'s manual serde
/// impl. `kind` is implied by position (see [`expected_kind`]) so it's
/// absent from the wire form; the encode side validates the convention
/// and the decode side reconstructs it.
#[derive(Serialize, Deserialize)]
struct WireCheckpoint {
    aabb: Aabb,
    label: Option<String>,
}

/// The position-implicit `CheckpointKind` rule shared by the encode
/// validator and the decode reconstructor: index 0 → `Start`, last
/// index → `End`, middle → `Split`. `SplitsState::step()` already
/// hard-relies on this layout (a misplaced `Start` would clear
/// `fired[]` mid-run; a `Split`/`End` at index 0 would never fire
/// because `next_index = 0` at load), so dropping the discriminant
/// from the wire form is a free byte saving rather than a relaxation
/// of behavior.
fn expected_kind(i: usize, n: usize) -> CheckpointKind {
    if i == 0 {
        CheckpointKind::Start
    } else if i + 1 == n {
        CheckpointKind::End
    } else {
        CheckpointKind::Split
    }
}

impl Serialize for Track {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let n = self.checkpoints.len();
        for (i, cp) in self.checkpoints.iter().enumerate() {
            let expected = expected_kind(i, n);
            if cp.kind != expected {
                return Err(S::Error::custom(format!(
                    "checkpoint[{i}] kind is {:?}, wire form requires {expected:?} \
                     (position-implicit: index 0 = Start, last = End, middle = Split)",
                    cp.kind
                )));
            }
        }
        let wires: Vec<WireCheckpoint> = self
            .checkpoints
            .iter()
            .map(|cp| WireCheckpoint {
                aabb: cp.aabb,
                label: cp.label.clone(),
            })
            .collect();
        (&self.name, &wires).serialize(s)
    }
}

impl<'de> Deserialize<'de> for Track {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let (name, wires): (String, Vec<WireCheckpoint>) = Deserialize::deserialize(d)?;
        let n = wires.len();
        let checkpoints = wires
            .into_iter()
            .enumerate()
            .map(|(i, w)| Checkpoint {
                kind: expected_kind(i, n),
                aabb: w.aabb,
                label: w.label,
            })
            .collect();
        Ok(Track { name, checkpoints })
    }
}

impl Track {
    /// Postcard-serialize, then zstd-compress at level 0. The output is
    /// the canonical wire form shared between the chat-protocol path and
    /// any future on-disk binary format.
    pub fn encode_to_wire(&self) -> Result<Vec<u8>> {
        let postcard_bytes = postcard::to_allocvec(self)?;
        Ok(zstd::encode_all(&*postcard_bytes, 0)?)
    }

    /// Reverse of [`encode_to_wire`]. The zstd decode runs in streaming
    /// mode with a [`MAX_DECOMPRESSED_BYTES`] cap so a hostile payload
    /// can't trigger unbounded allocation before the postcard step.
    pub fn decode_from_wire(wire: &[u8]) -> Result<Self> {
        let mut decoder = zstd::stream::Decoder::new(wire)?;
        let mut decompressed = Vec::new();
        // Read one byte past the cap so we can distinguish "exactly at cap"
        // (legitimate) from "would exceed cap" (bomb).
        decoder
            .by_ref()
            .take(MAX_DECOMPRESSED_BYTES as u64 + 1)
            .read_to_end(&mut decompressed)?;
        ensure!(
            decompressed.len() <= MAX_DECOMPRESSED_BYTES,
            "decompressed track exceeds {MAX_DECOMPRESSED_BYTES}-byte cap"
        );
        Ok(postcard::from_bytes(&decompressed)?)
    }
}

#[derive(Debug, Default)]
pub struct SplitsState {
    pub track: Option<Track>,
    pub next_index: usize,
    pub fired: Vec<bool>,
    pub last_inside: Vec<bool>,
}

impl SplitsState {
    pub fn load(&mut self, track: Track) {
        let n = track.checkpoints.len();
        self.track = Some(track);
        self.next_index = 0;
        self.fired = vec![false; n];
        self.last_inside = vec![false; n];
    }

    /// Re-arm the run without unloading the track. Called on
    /// `/client LiveSplit reset` and on `on_new_map_loaded`.
    pub fn rearm(&mut self) {
        self.next_index = 0;
        self.fired.fill(false);
        self.last_inside.fill(false);
    }
}

/// Pure decision function: given the current state and the player position
/// for this frame, advance the state and emit LiveSplit commands via `send`.
///
/// Rules:
/// - Edge-triggered: a checkpoint only fires on the outside-to-inside
///   transition, not while the player stands still inside it.
/// - Sequential: only `track.checkpoints[next_index]` is eligible to fire as
///   a Split / End. A Start box is always eligible (entering one re-arms
///   the run latch).
/// - One-shot: each box's `fired[i]` latches true until `rearm`.
pub fn step<F: FnMut(Command)>(state: &mut SplitsState, pos: Vec3, mut send: F) {
    let Some(track) = state.track.as_ref() else {
        return;
    };

    let inside_now: Vec<bool> = track
        .checkpoints
        .iter()
        .map(|cp| cp.aabb.contains(pos))
        .collect();

    for (i, cp) in track.checkpoints.iter().enumerate() {
        let entered = inside_now[i] && !state.last_inside[i];
        if !entered {
            continue;
        }

        match cp.kind {
            CheckpointKind::Start => {
                state.fired.iter_mut().for_each(|b| *b = false);
                state.fired[i] = true;
                state.next_index = i + 1;
                send(Command::Start);
            }
            CheckpointKind::Split | CheckpointKind::End
                if i == state.next_index && !state.fired[i] =>
            {
                state.fired[i] = true;
                state.next_index = i + 1;
                send(Command::Split);
            }
            _ => {}
        }
    }

    state.last_inside = inside_now;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f32, y: f32, z: f32) -> Vec3 {
        Vec3::new(x, y, z)
    }

    fn aabb(min: (f32, f32, f32), max: (f32, f32, f32)) -> Aabb {
        Aabb {
            min: v(min.0, min.1, min.2),
            max: v(max.0, max.1, max.2),
        }
    }

    fn cp(kind: CheckpointKind, min: (f32, f32, f32), max: (f32, f32, f32)) -> Checkpoint {
        Checkpoint {
            kind,
            aabb: aabb(min, max),
            label: None,
        }
    }

    fn linear_track() -> Track {
        Track {
            name: "linear".into(),
            checkpoints: vec![
                cp(CheckpointKind::Start, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0)),
                cp(CheckpointKind::Split, (10.0, 0.0, 0.0), (12.0, 4.0, 2.0)),
                cp(CheckpointKind::Split, (20.0, 0.0, 0.0), (22.0, 4.0, 2.0)),
                cp(CheckpointKind::End, (30.0, 0.0, 0.0), (32.0, 4.0, 2.0)),
            ],
        }
    }

    fn run(positions: &[Vec3]) -> Vec<Command> {
        let mut state = SplitsState::default();
        state.load(linear_track());
        let mut out = Vec::new();
        for p in positions {
            step(&mut state, *p, |c| out.push(c));
        }
        out
    }

    fn variant_name(c: &Command) -> &'static str {
        match c {
            Command::Start => "Start",
            Command::Split => "Split",
            _ => "Other",
        }
    }

    fn names(cmds: &[Command]) -> Vec<&'static str> {
        cmds.iter().map(variant_name).collect()
    }

    #[test]
    fn contains_point_inside() {
        assert!(aabb((0.0, 0.0, 0.0), (1.0, 1.0, 1.0)).contains(v(0.5, 0.5, 0.5)));
    }

    #[test]
    fn contains_min_corner_is_inside() {
        // Half-open: the min corner is included.
        assert!(aabb((0.0, 0.0, 0.0), (1.0, 1.0, 1.0)).contains(v(0.0, 0.0, 0.0)));
    }

    #[test]
    fn contains_max_corner_is_outside() {
        // Half-open: the max corner is excluded so adjacent boxes don't overlap.
        assert!(!aabb((0.0, 0.0, 0.0), (1.0, 1.0, 1.0)).contains(v(1.0, 1.0, 1.0)));
    }

    #[test]
    fn contains_just_outside_each_axis() {
        let b = aabb((0.0, 0.0, 0.0), (1.0, 1.0, 1.0));
        assert!(!b.contains(v(-0.1, 0.5, 0.5)));
        assert!(!b.contains(v(0.5, -0.1, 0.5)));
        assert!(!b.contains(v(0.5, 0.5, -0.1)));
        assert!(!b.contains(v(1.1, 0.5, 0.5)));
        assert!(!b.contains(v(0.5, 1.1, 0.5)));
        assert!(!b.contains(v(0.5, 0.5, 1.1)));
    }

    #[test]
    fn aabb_new_canonicalizes_swapped_corners() {
        let a = Aabb::new(v(5.0, 6.0, 7.0), v(1.0, 2.0, 3.0));
        let b = Aabb::new(v(1.0, 2.0, 3.0), v(5.0, 6.0, 7.0));
        assert_eq!(a, b);
        assert_eq!(a.min, v(1.0, 2.0, 3.0));
        assert_eq!(a.max, v(5.0, 6.0, 7.0));
    }

    #[test]
    fn sequential_traversal_fires_each_box_once() {
        let cmds = run(&[
            v(-5.0, 0.0, 0.0),
            v(1.0, 1.0, 1.0),  // Start
            v(11.0, 1.0, 1.0), // Split 1
            v(21.0, 1.0, 1.0), // Split 2
            v(31.0, 1.0, 1.0), // End
        ]);
        assert_eq!(names(&cmds), vec!["Start", "Split", "Split", "Split"]);
    }

    #[test]
    fn walking_back_through_earlier_split_does_not_refire() {
        let cmds = run(&[
            v(1.0, 1.0, 1.0),  // Start
            v(11.0, 1.0, 1.0), // Split 1
            v(-5.0, 0.0, 0.0), // leave
            v(11.0, 1.0, 1.0), // re-enter Split 1
        ]);
        assert_eq!(names(&cmds), vec!["Start", "Split"]);
    }

    #[test]
    fn standing_still_inside_box_fires_exactly_once() {
        let cmds = run(&[v(1.0, 1.0, 1.0), v(1.0, 1.0, 1.0), v(1.0, 1.0, 1.0)]);
        assert_eq!(names(&cmds), vec!["Start"]);
    }

    #[test]
    fn skipping_to_split_two_fires_nothing() {
        // Player skips Start and Split₁, jumps straight to Split₂.
        let cmds = run(&[v(-5.0, 0.0, 0.0), v(21.0, 1.0, 1.0)]);
        assert!(cmds.is_empty(), "expected no commands, got {cmds:?}");
    }

    #[test]
    fn end_without_first_hitting_splits_does_not_fire() {
        let cmds = run(&[v(-5.0, 0.0, 0.0), v(31.0, 1.0, 1.0)]);
        assert!(cmds.is_empty(), "expected no commands, got {cmds:?}");
    }

    #[test]
    fn reentering_start_rearms_the_run() {
        let mut state = SplitsState::default();
        state.load(linear_track());
        let mut cmds = Vec::new();

        let positions = [
            v(1.0, 1.0, 1.0),  // Start
            v(11.0, 1.0, 1.0), // Split 1
            v(21.0, 1.0, 1.0), // Split 2
            v(-5.0, 0.0, 0.0), // leave
            v(1.0, 1.0, 1.0),  // back to Start → re-arm
        ];
        for p in &positions {
            step(&mut state, *p, |c| cmds.push(c));
        }

        assert_eq!(names(&cmds), vec!["Start", "Split", "Split", "Start"]);
        assert_eq!(state.next_index, 1);
        assert_eq!(state.fired, vec![true, false, false, false]);
    }

    #[test]
    fn no_commands_when_no_track_loaded() {
        let mut state = SplitsState::default();
        let mut cmds = Vec::new();
        step(&mut state, v(0.0, 0.0, 0.0), |c| cmds.push(c));
        assert!(cmds.is_empty());
    }

    #[test]
    fn rearm_clears_fired_and_resets_cursor() {
        let mut state = SplitsState::default();
        state.load(linear_track());
        step(&mut state, v(1.0, 1.0, 1.0), |_| {});
        step(&mut state, v(11.0, 1.0, 1.0), |_| {});
        assert_eq!(state.next_index, 2);
        assert_eq!(state.fired, vec![true, true, false, false]);

        state.rearm();
        assert_eq!(state.next_index, 0);
        assert_eq!(state.fired, vec![false; 4]);
        assert_eq!(state.last_inside, vec![false; 4]);
    }

    #[test]
    fn track_round_trips_through_wire_with_implicit_kind() {
        let original = linear_track();
        let wire = original.encode_to_wire().unwrap();
        let decoded = Track::decode_from_wire(&wire).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn track_round_trips_two_checkpoint_track() {
        let original = Track {
            name: "minimal".into(),
            checkpoints: vec![
                cp(CheckpointKind::Start, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0)),
                cp(CheckpointKind::End, (10.0, 0.0, 0.0), (12.0, 4.0, 2.0)),
            ],
        };
        let wire = original.encode_to_wire().unwrap();
        let decoded = Track::decode_from_wire(&wire).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn track_serialize_rejects_misplaced_start() {
        let bad = Track {
            name: "bad".into(),
            checkpoints: vec![
                cp(CheckpointKind::Start, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0)),
                cp(CheckpointKind::Start, (10.0, 0.0, 0.0), (12.0, 4.0, 2.0)),
                cp(CheckpointKind::End, (20.0, 0.0, 0.0), (22.0, 4.0, 2.0)),
            ],
        };
        assert!(bad.encode_to_wire().is_err());
    }

    #[test]
    fn track_serialize_rejects_misplaced_end() {
        let bad = Track {
            name: "bad".into(),
            checkpoints: vec![
                cp(CheckpointKind::End, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0)),
                cp(CheckpointKind::Split, (10.0, 0.0, 0.0), (12.0, 4.0, 2.0)),
                cp(CheckpointKind::End, (20.0, 0.0, 0.0), (22.0, 4.0, 2.0)),
            ],
        };
        assert!(bad.encode_to_wire().is_err());
    }

    #[test]
    fn track_serialize_rejects_split_at_index_zero() {
        let bad = Track {
            name: "bad".into(),
            checkpoints: vec![
                cp(CheckpointKind::Split, (0.0, 0.0, 0.0), (2.0, 4.0, 2.0)),
                cp(CheckpointKind::End, (10.0, 0.0, 0.0), (12.0, 4.0, 2.0)),
            ],
        };
        assert!(bad.encode_to_wire().is_err());
    }
}
