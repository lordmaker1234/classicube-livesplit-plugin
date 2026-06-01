#[cfg(test)]
mod tests;

use std::{fs, path::PathBuf};

use classicube_helpers::tab_list::remove_color;

/// Root directory for the on-disk track store, relative to the
/// process working directory (which on ClassiCube is the install
/// dir — the same place the game itself looks for
/// `./plugins/<libname>.{so,dll,dylib}`).
const ROOT: &str = "plugins/livesplit";

const MAX_COMPONENT_LEN: usize = 64;

/// Lowercase the path-unsafe surface of a display string into a
/// filename-safe slug. Applied to all three path components
/// (`<server>`, `<map>`, `<category>`):
///
///   1. strip ClassiCube color codes (every `&` + next char dropped pair-wise)
///   2. replace anything outside `[A-Za-z0-9._-]` with `_`
///   3. collapse runs of `_`
///   4. trim leading/trailing `_`
///   5. cap at 64 characters (at a UTF-8 char boundary)
///
/// Returns `"_"` if the result would otherwise be empty so the path
/// stays well-formed (`fs::create_dir_all` is happy, and the
/// surrounding code doesn't have to special-case empties).
pub fn sanitize_component(s: &str) -> String {
    let stripped = remove_color(s);

    let mapped: String = stripped
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();

    let collapsed = collapse_underscores(&mapped);
    let trimmed = collapsed.trim_matches('_');

    let capped: String = trimmed.chars().take(MAX_COMPONENT_LEN).collect();
    // Cap may have re-introduced a trailing `_` if the boundary fell
    // mid-run (it can't here because `collapse_underscores` already
    // collapsed runs to single `_`, but trim again defensively).
    let capped = capped.trim_matches('_').to_owned();

    if capped.is_empty() {
        "_".to_owned()
    } else {
        capped
    }
}

fn collapse_underscores(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_underscore = false;
    for c in s.chars() {
        if c == '_' {
            if !prev_underscore {
                out.push('_');
                prev_underscore = true;
            }
        } else {
            out.push(c);
            prev_underscore = false;
        }
    }
    out
}

/// Build the directory the writer should create / the reader should
/// scan for `(server, map)`. Each component is sanitized.
pub fn track_dir(server: &str, map: &str) -> PathBuf {
    PathBuf::from(ROOT)
        .join(sanitize_component(server))
        .join(sanitize_component(map))
}

/// Scan `dir` for files matching `<category>-v<N>.lss` and return
/// them sorted ascending by version number. Silently skips non-`.lss`
/// entries, mismatched prefixes, and unparseable version numbers.
/// Returns an empty vec if the directory doesn't exist.
pub fn list_versions(dir: &std::path::Path, category: &str) -> Vec<(u32, PathBuf)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("lss") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(version_part) = stem.strip_prefix(&format!("{category}-v")) else {
            continue;
        };
        let Ok(v) = version_part.parse::<u32>() else {
            continue;
        };
        out.push((v, path));
    }
    out.sort_by_key(|(v, _)| *v);
    out
}
