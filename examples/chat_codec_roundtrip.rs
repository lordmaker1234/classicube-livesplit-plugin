//! Interactive round-trip validator for the `chat_codec` module.
//!
//! Two subcommands:
//!
//! - `edges` (default): walks a small fixed set of edge-case payloads. For
//!   each, prints a `/mb sign <encoded>` command. Run it in-game, walk into
//!   the message block to trigger the echo, copy the blob portion of the
//!   echoed chat line(s) (strip your username manually), paste it back,
//!   and finish with an empty line. Bin strips leading `&X` color codes
//!   and inline `> &X` continuation markers, then decodes and compares
//!   against the original payload.
//!
//! - `probe [lo hi]`: narrows the largest byte payload that survives the
//!   `/mb sign` round-trip. ClassiCube's chat input caps at
//!   `INPUTWIDGET_MAX_LINES * INPUTWIDGET_LEN = 3 * 64 = 192` codepoints
//!   (Widgets.h:124-125 & 237); subtracting the 9-char `/mb sign ` prefix
//!   leaves `MAX_BLOB_CODEPOINTS = 183` for the encoded blob. Anything
//!   above that is short-circuited as a fail without prompting — you
//!   physically can't type it. For the deterministic probe seed the
//!   encoding ratio is steady at `cp = bytes + 3`, so 180 bytes (183 cp)
//!   fits exactly and 181 bytes (184 cp) doesn't. Walks `lo..=hi`
//!   linearly, stopping at the first failure. Defaults to `175 184`.

use std::io::{self, BufRead, Write};

use classicube_livesplit_plugin::chat_codec::{decode, encode};

const COMMAND_PREFIX: &str = "/mb sign ";

/// ClassiCube chat input buffer capacity in codepoints:
/// `INPUTWIDGET_MAX_LINES * INPUTWIDGET_LEN` from Widgets.h:124-125 & 237.
/// The `InputWidget_TryAppendChar` cap enforcement is at Widgets.c:1259-1260.
const CHAT_INPUT_CAPACITY: usize = 192;

/// Max codepoints we'll ask the user to type as the blob argument to
/// `/mb sign`. Anything longer can't be appended into the chat input.
const MAX_BLOB_CODEPOINTS: usize = CHAT_INPUT_CAPACITY - COMMAND_PREFIX.len();

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(String::as_str).unwrap_or("edges");
    match mode {
        "edges" => run_edges(),
        "probe" => run_probe(args.get(2..).unwrap_or_default()),
        "sizes" => run_sizes(args.get(2..).unwrap_or_default()),
        _ => {
            eprintln!("usage: chat_codec_roundtrip [edges|probe [lo hi]|sizes [lo hi]]");
            std::process::exit(2);
        }
    }
}

/// Print `byte_len -> encoded codepoints` for a range of input lengths.
/// Useful for sizing the chat-input cliff against the probe's deterministic
/// payload (the encoding ratio fluctuates by ±1 cp depending on content).
fn run_sizes(extra: &[String]) {
    let lo: usize = extra.first().and_then(|s| s.parse().ok()).unwrap_or(170);
    let hi: usize = extra.get(1).and_then(|s| s.parse().ok()).unwrap_or(190);
    for n in lo..=hi {
        let cp = encode(&deterministic(n)).chars().count();
        println!("{:>4} bytes -> {:>4} codepoints", n, cp);
    }
}

/// Deterministic byte stream — small PRNG over `u8`, no casts.
fn deterministic(byte_len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(byte_len);
    let mut state: u8 = 13;
    for _ in 0..byte_len {
        state = state.wrapping_mul(37).wrapping_add(7);
        out.push(state);
    }
    out
}

/// Read paste lines from stdin until an empty line / EOF; concatenate and
/// strip MCGalaxy chat markup. The empty-line terminator lets users paste
/// multi-line wrapped chat output.
fn read_paste(stdin: &mut impl BufRead, line: &mut String) -> String {
    let mut pasted = String::new();
    loop {
        line.clear();
        if stdin.read_line(line).unwrap_or(0) == 0 {
            break;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        pasted.push_str(trimmed);
    }
    strip_chat_markup(&pasted)
}

/// Strip the chat markup MCGalaxy / LineWrapper add around blob bytes:
///
/// - one leading `&X` color code (`X` = any one char) at the very start of
///   the paste — the per-line color prefix on the first echoed line;
/// - every inline `> &X` sequence — LineWrapper's continuation marker on
///   every wrapped line past the first (ClassiCube's incoming chat caps at
///   64 chars, so any non-trivial blob wraps).
fn strip_chat_markup(input: &str) -> String {
    let mut rest = input;
    if let Some(after_amp) = rest.strip_prefix('&') {
        let mut chars = after_amp.chars();
        if chars.next().is_some() {
            rest = chars.as_str();
        }
    }
    let mut out = String::with_capacity(rest.len());
    while !rest.is_empty() {
        if let Some(after_marker) = rest.strip_prefix("> &") {
            let mut chars = after_marker.chars();
            if chars.next().is_some() {
                rest = chars.as_str();
                continue;
            }
        }
        let mut chars = rest.chars();
        if let Some(c) = chars.next() {
            out.push(c);
        }
        rest = chars.as_str();
    }
    out
}

fn run_edges() {
    let payloads: Vec<(&str, Vec<u8>)> = vec![
        ("empty", vec![]),
        ("single 0x00", vec![0x00]),
        ("single 0xFF", vec![0xFF]),
        ("leading zeros [0,0,0,1,2,3]", vec![0, 0, 0, 1, 2, 3]),
        ("all bytes 0x00..=0xFF", (0u8..=255).collect()),
        ("64-byte deterministic", deterministic(64)),
    ];

    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    let mut line = String::new();

    let mut pass = 0u32;
    let mut fail = 0u32;
    let mut skip = 0u32;

    for (name, payload) in &payloads {
        let encoded = encode(payload);
        let cp = encoded.chars().count();
        println!("\n{}", "-".repeat(72));
        println!("payload: {} ({} bytes)", name, payload.len());
        println!("encoded: {} bytes / {cp} codepoints", encoded.len());

        if encoded.is_empty() {
            println!("(empty encoding — nothing to /mb sign, skipping)");
            skip += 1;
            continue;
        }
        if cp > MAX_BLOB_CODEPOINTS {
            println!(
                "(encoded {cp} cp > {MAX_BLOB_CODEPOINTS} chat-input cap — can't be typed, \
                 skipping)"
            );
            skip += 1;
            continue;
        }

        println!();
        println!("    {COMMAND_PREFIX}{encoded}");
        println!();
        println!("paste the echoed blob, finish with an empty line. blank line = skip:");
        print!("> ");
        io::stdout().flush().ok();

        let pasted = read_paste(&mut stdin, &mut line);
        if pasted.is_empty() {
            println!("[skip]");
            skip += 1;
            continue;
        }

        match decode(&pasted) {
            Ok(d) if d == *payload => {
                println!("[pass] decoded matches original");
                pass += 1;
            }
            Ok(d) => {
                let common = payload.iter().zip(&d).take_while(|(a, b)| a == b).count();
                println!(
                    "[fail] mismatch: expected {} bytes, got {} ({} bytes agree)",
                    payload.len(),
                    d.len(),
                    common
                );
                fail += 1;
            }
            Err(e) => {
                println!("[fail] decode error: {}", e);
                fail += 1;
            }
        }
    }

    println!("\n{}", "=".repeat(72));
    println!("{} pass, {} fail, {} skip", pass, fail, skip);
    if fail > 0 {
        std::process::exit(1);
    }
}

/// Run one probe step at the given byte length. Returns true iff the pasted
/// echo decodes back to the deterministic payload of that length.
fn step(stdin: &mut impl BufRead, line: &mut String, byte_len: usize) -> bool {
    let payload = deterministic(byte_len);
    let encoded = encode(&payload);
    let cp = encoded.chars().count();
    println!("\n{}", "-".repeat(72));
    println!(
        ">>> trying {byte_len} bytes payload -> {} bytes / {cp} codepoints encoded",
        encoded.len()
    );
    if cp > MAX_BLOB_CODEPOINTS {
        println!(
            "    {cp} cp > {MAX_BLOB_CODEPOINTS} chat-input cap — can't be typed, treating as fail"
        );
        return false;
    }
    println!();
    println!("    {COMMAND_PREFIX}{encoded}");
    println!();
    println!("paste the echoed blob, end with an empty line:");
    print!("> ");
    io::stdout().flush().ok();

    let pasted = read_paste(stdin, line);
    match decode(&pasted) {
        Ok(d) => d == payload,
        Err(_) => false,
    }
}

fn run_probe(extra: &[String]) {
    let lo: usize = extra.first().and_then(|s| s.parse().ok()).unwrap_or(175);
    let hi: usize = extra.get(1).and_then(|s| s.parse().ok()).unwrap_or(184);

    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    let mut line = String::new();

    println!("probe: narrowing the byte-payload cliff. walking {lo}..={hi} linearly,");
    println!("       stopping at the first FAIL.");

    let mut last_pass: Option<usize> = None;
    for n in lo..=hi {
        let ok = step(&mut stdin, &mut line, n);
        println!("[{n} bytes] {}", if ok { "PASS" } else { "FAIL" });
        if ok {
            last_pass = Some(n);
        } else {
            break;
        }
    }

    println!("\n{}", "=".repeat(72));
    match last_pass {
        Some(n) => {
            let cp = encode(&deterministic(n)).chars().count();
            println!("max byte-payload that round-trips in {lo}..={hi}: {n} bytes");
            println!("    ({cp} encoded codepoints at that length)");
        }
        None => println!("no payload in {lo}..={hi} round-tripped"),
    }
}
