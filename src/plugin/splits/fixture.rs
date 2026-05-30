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

    #[test]
    fn print_loadtest_encode_breakdown() {
        use crate::chat_codec;

        let track = loadtest();
        let postcard_bytes = postcard::to_allocvec(&track).unwrap();
        let zstd_bytes = zstd::encode_all(&*postcard_bytes, 0).unwrap();
        let encoded = chat_codec::encode(&zstd_bytes);
        let line = format!("&m{encoded}");

        eprintln!("postcard: {} bytes", postcard_bytes.len());
        eprintln!("zstd:     {} bytes", zstd_bytes.len());
        eprintln!("encoded:  {} cp", encoded.chars().count());
        eprintln!("line:     {} cp (with &m)", line.chars().count());
    }

    /// Explore whether a pre-shared zstd dictionary lowers wire size for
    /// `Track` payloads. A dictionary baked into the plugin binary doesn't
    /// count against the chat-line budget — only the dict-compressed
    /// frame does — so the relevant comparison is "zstd frame with dict"
    /// vs the current "zstd frame, no dict".
    ///
    /// Trains on a synthetic corpus of plausible tracks (varying name,
    /// count, coords, label patterns), then encodes the loadtest fixture
    /// with several dict sizes and zstd levels.
    #[test]
    fn print_dict_size_sweep() {
        use zstd::{bulk::Compressor, dict::from_samples};

        let corpus = synthetic_corpus();
        let samples: Vec<Vec<u8>> = corpus
            .iter()
            .map(|t| postcard::to_allocvec(t).unwrap())
            .collect();
        let corpus_total: usize = samples.iter().map(Vec::len).sum();

        let track = loadtest();
        let postcard_bytes = postcard::to_allocvec(&track).unwrap();
        let plain_zstd = zstd::encode_all(&*postcard_bytes, 0).unwrap();

        eprintln!(
            "corpus: {} samples, {corpus_total} postcard bytes total",
            samples.len()
        );
        eprintln!("loadtest postcard: {} bytes", postcard_bytes.len());
        eprintln!("loadtest zstd (no dict, lvl 0): {} bytes", plain_zstd.len());
        eprintln!();
        eprintln!("dict_size  level   wire_bytes   delta_vs_no_dict");

        for &max_dict in &[
            128_usize, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536,
        ] {
            let dict = match from_samples(&samples, max_dict) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("{max_dict:>9}    --   skipped: {e}");
                    continue;
                }
            };

            for &level in &[1, 3, 9, 19, 22] {
                let mut c = Compressor::with_dictionary(level, &dict).unwrap();
                let out = c.compress(&postcard_bytes).unwrap();
                let delta = out.len() as i32 - plain_zstd.len() as i32;
                eprintln!(
                    "{:>9}  {:>5}   {:>10}   {:+}   (dict actual {} B)",
                    max_dict,
                    level,
                    out.len(),
                    delta,
                    dict.len(),
                );
            }
        }

        eprintln!();
        eprintln!("for reference, varying level WITHOUT a dict:");
        for &level in &[1, 3, 9, 19, 22] {
            let out = zstd::encode_all(&*postcard_bytes, level).unwrap();
            eprintln!("  level {level:>2}: {} bytes", out.len());
        }

        // For the chat-line / goodlyay2 breakdown, show two reference
        // dict sizes: a small 512-byte dict (cheap binary bloat, modest
        // win) and the empirical 16 KB sweet spot we saw above (bigger
        // bloat, biggest win for the loadtest payload).
        for dict_cap in [512_usize, 16384] {
            let dict = from_samples(&samples, dict_cap).unwrap();
            eprintln!();
            eprintln!(
                "loadtest → chat-codec line length (dict cap {dict_cap}, actual {} B):",
                dict.len()
            );
            for &level in &[9, 19] {
                let mut c = Compressor::with_dictionary(level, &dict).unwrap();
                let out = c.compress(&postcard_bytes).unwrap();
                let encoded = crate::chat_codec::encode(&out);
                let line_cp = 2 + encoded.chars().count();
                eprintln!(
                    "  lvl {level:>2}: wire {} B, encoded {} cp, &m-line {} cp (cap 61)",
                    out.len(),
                    encoded.chars().count(),
                    line_cp,
                );
            }

            // Real-world goodlyay2 track — far larger than loadtest, so
            // the chat-line cap is structurally out of reach. Reported
            // anyway to illustrate the gap.
            let goodlyay2 = corpus
                .iter()
                .find(|t| t.name == "Not Awesome 2 - goodlyay2")
                .expect("goodlyay2 fixture present in corpus");
            let g2_postcard = postcard::to_allocvec(goodlyay2).unwrap();
            let g2_zstd_baseline = zstd::encode_all(&*g2_postcard, 0).unwrap();
            eprintln!(
                "goodlyay2 (17 cps, postcard {} B, zstd-no-dict {} B):",
                g2_postcard.len(),
                g2_zstd_baseline.len(),
            );
            for &level in &[9, 19] {
                let mut c = Compressor::with_dictionary(level, &dict).unwrap();
                let out = c.compress(&g2_postcard).unwrap();
                let encoded = crate::chat_codec::encode(&out);
                let line_cp = 2 + encoded.chars().count();
                eprintln!(
                    "  lvl {level:>2}: wire {} B, encoded {} cp, &m-line {} cp (cap 61)",
                    out.len(),
                    encoded.chars().count(),
                    line_cp,
                );
            }
        }
    }

    /// Train one dict per domain (Not Awesome 2 real names, generic
    /// synthetic names, `split N` pattern) plus a combined dict, then
    /// encode several test payloads with each and report which dict
    /// won. This is the realistic shipping strategy: bake N specialized
    /// dicts into the plugin, let the encoder pick the smallest per
    /// payload, and embed the winning dict's ID into the frame header
    /// so the decoder knows which to load.
    #[test]
    fn print_multi_dict_strategy() {
        use zstd::{bulk::Compressor, dict::from_samples};

        const DICT_CAP: usize = 16384;
        const LEVEL: i32 = 9;

        let domains: Vec<(&'static str, Vec<Track>)> = vec![
            ("not_awesome_2", not_awesome_2_corpus()),
            ("generic", generic_corpus()),
            ("split_n", split_n_corpus()),
        ];

        struct TrainedDict {
            name: &'static str,
            bytes: Vec<u8>,
        }
        let mut trained: Vec<TrainedDict> = Vec::new();
        for (name, corpus) in &domains {
            let samples: Vec<Vec<u8>> = corpus
                .iter()
                .map(|t| postcard::to_allocvec(t).unwrap())
                .collect();
            let corpus_bytes: usize = samples.iter().map(Vec::len).sum();
            let dict = from_samples(&samples, DICT_CAP).unwrap();
            eprintln!(
                "  trained dict {:<14} from {:>3} samples / {:>5} B → actual dict {:>5} B",
                name,
                samples.len(),
                corpus_bytes,
                dict.len(),
            );
            trained.push(TrainedDict { name, bytes: dict });
        }

        let all_samples: Vec<Vec<u8>> = synthetic_corpus()
            .iter()
            .map(|t| postcard::to_allocvec(t).unwrap())
            .collect();
        let combined_dict = from_samples(&all_samples, DICT_CAP).unwrap();
        eprintln!(
            "  trained dict {:<14} from {:>3} samples / {:>5} B → actual dict {:>5} B",
            "combined",
            all_samples.len(),
            all_samples.iter().map(Vec::len).sum::<usize>(),
            combined_dict.len(),
        );

        // Test payloads — one from each domain plus the existing
        // loadtest fixture, to expose whether the per-domain dicts
        // really win on their home turf.
        let goodlyay2 = make_track("Not Awesome 2 - goodlyay2", &not_awesome_2_pool(), 42);
        let generic_route = make_track(
            "generic any%",
            &["hub", "tower", "boss", "boss2", "finale"]
                .iter()
                .map(|s| (*s).into())
                .collect::<Vec<_>>(),
            7,
        );
        let payloads: Vec<(&str, Track)> = vec![
            ("loadtest (split_n)", loadtest()),
            ("goodlyay2 (na2)", goodlyay2),
            ("generic route (generic)", generic_route),
        ];

        eprintln!();
        eprintln!("per-payload: each row is one compression choice, smallest wins");
        for (label, track) in &payloads {
            let postcard_bytes = postcard::to_allocvec(track).unwrap();
            let no_dict = zstd::encode_all(&*postcard_bytes, LEVEL).unwrap();

            let mut rows: Vec<(&'static str, usize)> = vec![("(no dict)", no_dict.len())];
            for td in &trained {
                let mut c = Compressor::with_dictionary(LEVEL, &td.bytes).unwrap();
                let out = c.compress(&postcard_bytes).unwrap();
                rows.push((td.name, out.len()));
            }
            let mut c = Compressor::with_dictionary(LEVEL, &combined_dict).unwrap();
            let combined_out = c.compress(&postcard_bytes).unwrap();
            rows.push(("combined", combined_out.len()));

            let winner = rows.iter().min_by_key(|(_, n)| *n).unwrap().0;

            eprintln!();
            eprintln!(
                "{label} — postcard {} B, baseline no-dict zstd {} B",
                postcard_bytes.len(),
                no_dict.len()
            );
            for (dict_name, wire) in &rows {
                let mark = if *dict_name == winner {
                    "  ← winner"
                } else {
                    ""
                };
                eprintln!("  {dict_name:<14} → wire {wire:>4} B{mark}");
            }
        }

        // Sanity check: every dict round-trips its own frame. Decoder
        // would look at the frame's dict ID and load the matching dict;
        // here we just simulate by feeding the right dict back.
        for td in &trained {
            let payload = postcard::to_allocvec(&loadtest()).unwrap();
            let mut c = Compressor::with_dictionary(LEVEL, &td.bytes).unwrap();
            let wire = c.compress(&payload).unwrap();
            let mut d = zstd::bulk::Decompressor::with_dictionary(&td.bytes).unwrap();
            // Same 16-KiB cap geometry.rs uses for the chat-wire path.
            let roundtripped = d.decompress(&wire, 16 * 1024).unwrap();
            assert_eq!(roundtripped, payload, "{} dict round-trip", td.name);
        }
    }

    /// Real user data: segment names lifted from the
    /// `Not Awesome 2 - goodlyay2.lss` reference file. The first
    /// segment in LiveSplit is the first thing the runner crosses after
    /// the implicit Start, so it lands as the first Split label here;
    /// the last segment becomes the End label.
    fn not_awesome_2_pool() -> Vec<String> {
        [
            "goodlyay2",
            "desert",
            "basement",
            "basement2",
            "arctic",
            "strangehouse",
            "darkplace",
            "summer",
            "town",
            "nowhere",
            "kidnapped",
            "kidnapped2",
            "fog",
            "ridge",
            "penthouse",
            "bridge",
            "landing",
        ]
        .iter()
        .map(|s| (*s).into())
        .collect()
    }

    /// Larger pool of plausible ClassiCube map / segment names so a
    /// dict trained on it learns short-lowercase-token structure
    /// generally, not just one specific game's vocabulary.
    fn generic_map_pool() -> Vec<String> {
        [
            "hub",
            "tower",
            "maze",
            "cave",
            "ruins",
            "forest",
            "ocean",
            "void",
            "skybridge",
            "lava",
            "ice",
            "sand",
            "fortress",
            "mines",
            "village",
            "factory",
            "rooftop",
            "tunnel",
            "abyss",
            "garden",
            "temple",
            "cliff",
            "spire",
            "graveyard",
            "swamp",
            "marsh",
            "highway",
            "city",
            "outpost",
            "vault",
            "cellar",
            "attic",
            "boss",
            "boss2",
            "finale",
            "credits",
            "world1",
            "world1-2",
            "world2",
            "world2-3",
            "world3",
            "world3-4",
            "world4",
            "checkpoint",
            "start",
            "midway",
            "shortcut",
            "skip",
            "glitch",
        ]
        .iter()
        .map(|s| (*s).into())
        .collect()
    }

    /// Corpus of tracks built from the real `Not Awesome 2 - goodlyay2`
    /// `.lss` segment names: the goodlyay2 track itself, a handful of
    /// hand-authored category variations (any% / low% / 100% / sub-set
    /// routes), and 120 deterministic-pseudo-random subsets of the same
    /// pool.
    fn not_awesome_2_corpus() -> Vec<Track> {
        let pool = not_awesome_2_pool();
        let mut out = Vec::new();

        out.push(make_track("Not Awesome 2 - goodlyay2", &pool, 42));

        let variations: &[(&str, &[&str])] = &[
            (
                "Not Awesome 2 - any%",
                &[
                    "goodlyay2",
                    "desert",
                    "arctic",
                    "town",
                    "kidnapped",
                    "ridge",
                    "landing",
                ],
            ),
            (
                "Not Awesome 2 - low%",
                &[
                    "goodlyay2",
                    "basement",
                    "basement2",
                    "darkplace",
                    "fog",
                    "landing",
                ],
            ),
            (
                "Not Awesome 2 - 100%",
                &[
                    "goodlyay2",
                    "desert",
                    "basement",
                    "basement2",
                    "arctic",
                    "strangehouse",
                    "darkplace",
                    "summer",
                    "town",
                    "nowhere",
                    "kidnapped",
                    "kidnapped2",
                    "fog",
                    "ridge",
                    "penthouse",
                    "bridge",
                    "landing",
                ],
            ),
            (
                "Not Awesome 2 - basement",
                &["goodlyay2", "basement", "basement2"],
            ),
            (
                "Not Awesome 2 - kidnapped",
                &["goodlyay2", "kidnapped", "kidnapped2"],
            ),
        ];
        for (i, (name, labels)) in variations.iter().enumerate() {
            let labels: Vec<String> = labels.iter().map(|s| (*s).into()).collect();
            out.push(make_track(name, &labels, 100 + i as u16));
        }

        for sample_idx in 0..120 {
            let len = 3 + (sample_idx % 12);
            let labels = take_pseudo(&pool, len, sample_idx as u32);
            out.push(make_track(
                &format!("na2-synth {sample_idx}"),
                &labels,
                200_u16.wrapping_add(sample_idx as u16),
            ));
        }
        out
    }

    /// Corpus of tracks drawn from the made-up generic ClassiCube map
    /// pool. 120 deterministic-pseudo-random subsets, varying length
    /// and ordering.
    fn generic_corpus() -> Vec<Track> {
        let pool = generic_map_pool();
        let mut out = Vec::new();
        for sample_idx in 0..120 {
            let len = 3 + (sample_idx % 12);
            let labels = take_pseudo(&pool, len, sample_idx as u32);
            out.push(make_track(
                &format!("generic-synth {sample_idx}"),
                &labels,
                500_u16.wrapping_add(sample_idx as u16),
            ));
        }
        out
    }

    /// Corpus of tracks using the bare-bones `split N` labeling pattern
    /// — what a user gets if they don't bother naming checkpoints.
    fn split_n_corpus() -> Vec<Track> {
        let generic_names = [
            "loadtest",
            "doubletower",
            "skyland",
            "parkour1",
            "speedmap",
            "freebuild",
            "lavaclimb",
            "icerun",
            "nostalgia",
            "shortcut",
        ];
        let mut out = Vec::new();
        for (i, name) in generic_names.iter().enumerate() {
            let count = 3 + (i % 8);
            let labels: Vec<String> = (1..=count).map(|k| format!("split {k}")).collect();
            out.push(make_track(name, &labels, i as u16));
        }
        out
    }

    /// Combined corpus for the single-dict sweep test: all three
    /// domains concatenated.
    fn synthetic_corpus() -> Vec<Track> {
        let mut out = split_n_corpus();
        out.extend(not_awesome_2_corpus());
        out.extend(generic_corpus());
        out
    }

    /// Deterministic pseudo-random subset of `pool` of length `n`,
    /// driven by a tiny LCG. Used only for dict-training corpus
    /// generation — no real randomness needed; we just want spread.
    fn take_pseudo(pool: &[String], n: usize, seed: u32) -> Vec<String> {
        let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
        let mut step = || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            state
        };
        let mut taken = Vec::with_capacity(n);
        for _ in 0..n {
            let idx = (step() as usize) % pool.len();
            taken.push(pool[idx].clone());
        }
        taken
    }

    /// Build a Track with one Start (label "start"), one Split per entry
    /// in `splits` (the last one promoted to End), and plausible
    /// non-overlapping AABBs spaced along +X.
    fn make_track(name: &str, splits: &[String], seed: u16) -> Track {
        let stride: u16 = 8 + (seed % 5) * 2;
        let base_y = (seed % 10) * 4;
        let base_z = (seed % 7) * 3;
        let mut cps = Vec::with_capacity(splits.len() + 1);
        cps.push(checkpoint(
            CheckpointKind::Start,
            (0.0, f32::from(base_y), f32::from(base_z)),
            (2.0, f32::from(base_y) + 4.0, f32::from(base_z) + 2.0),
            "start",
        ));
        for (k, label) in splits.iter().enumerate() {
            let kind = if k + 1 == splits.len() {
                CheckpointKind::End
            } else {
                CheckpointKind::Split
            };
            let x = f32::from((k as u16 + 1) * stride);
            cps.push(checkpoint(
                kind,
                (x, f32::from(base_y), f32::from(base_z)),
                (x + 2.0, f32::from(base_y) + 4.0, f32::from(base_z) + 2.0),
                label,
            ));
        }
        Track {
            name: name.into(),
            checkpoints: cps,
        }
    }

    #[test]
    fn save_loadtest_as_lss() {
        use std::{env, fs};

        use livesplit_core::{Run, Segment, run::saver::livesplit::save_run};

        let mut run = Run::new();
        run.set_game_name("ClassiCube");
        run.set_category_name("loadtest");

        // LiveSplit's segment list is everything after the implicit Start —
        // pressing Start is the timer-side action, not a named segment. So
        // the fixture's Start checkpoint doesn't get a Segment; the rest do.
        let segment_names = ["split 1", "split 2", "end"];
        for name in segment_names {
            run.push_segment(Segment::new(name));
        }

        let mut buf = String::new();
        save_run(&run, &mut buf).unwrap();

        let path = env::temp_dir().join("loadtest.lss");
        fs::write(&path, &buf).unwrap();
        eprintln!("wrote {} bytes to {}", buf.len(), path.display());

        assert!(buf.starts_with(r#"<?xml version="1.0" encoding="UTF-8"?>"#));
        assert!(buf.contains(r#"<Run version="1.8.0">"#));
        assert!(buf.contains("<GameName>ClassiCube</GameName>"));
        assert!(buf.contains("<CategoryName>loadtest</CategoryName>"));
        for name in segment_names {
            assert!(
                buf.contains(&format!("<Name>{name}</Name>")),
                "missing segment <Name>{name}</Name> in:\n{buf}"
            );
        }
    }
}
