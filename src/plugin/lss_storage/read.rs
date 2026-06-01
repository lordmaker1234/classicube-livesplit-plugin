use std::{fs, path::Path, time::SystemTime};

use anyhow::{Context, Result, bail};
use classicube_helpers::async_manager;
use livesplit_core::Run;
use tracing::{debug, info};

use crate::{
    chat::chat_print_async,
    plugin::{
        lss_storage::{
            path,
            payload::{self, CUSTOM_VAR_NAME},
        },
        splits::{self, geometry::Track},
    },
};

/// Attempt to auto-load the most recently modified `.lss` file from
/// the on-disk store for `(server, map)`. Soft-fails on missing
/// directory; chat-prints on parse failures so user-introduced errors
/// (e.g. hand-edited custom variable) surface.
pub async fn try_autoload(server: String, map: String) {
    match try_autoload_inner(&server, &map) {
        Ok(None) => {
            debug!("autoload: no track files for ({server}, {map})");
        }
        Ok(Some(track)) => {
            async_manager::spawn_on_main_thread(async move {
                // Re-check on the main thread: the disk read + thread
                // hop is a sub-second window in which the player can
                // step into a Start AABB, which would make the
                // autoload clobber a just-started run
                // (`SplitsState::load` resets `next_index`/`fired[]`).
                if splits::run_in_progress() {
                    debug!("autoload skipped on main thread: run started during disk read");
                    return;
                }
                // `false` means the plugin is mid-teardown
                // (`SplitsState::load` returned `None` because `STATE`
                // was cleared); nothing actionable beyond logging.
                if !splits::load_track(track, "disk") {
                    debug!("autoload: load_track returned false (plugin mid-teardown)");
                }
            });
        }
        Err(e) => {
            debug!("autoload skipped: {e:#}");
        }
    }
}

fn try_autoload_inner(server: &str, map: &str) -> Result<Option<Track>> {
    let dir = path::track_dir(server, map);
    if !dir.exists() {
        return Ok(None);
    }

    let Some(newest) = newest_lss(&dir)? else {
        return Ok(None);
    };

    match load_from_file(&newest) {
        Ok(track) => {
            info!(path = %newest.display(), "auto-loaded track from disk");
            Ok(Some(track))
        }
        Err(e) => {
            let basename = newest.file_name().map_or_else(
                || newest.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            chat_print_async(format!("&eLiveSplit: skipping {basename}: {e:#}"));
            Ok(None)
        }
    }
}

fn newest_lss(dir: &Path) -> Result<Option<std::path::PathBuf>> {
    let entries = fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?;
    let mut best: Option<(SystemTime, std::path::PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("lss") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(mtime) = meta.modified() else { continue };
        match best {
            Some((cur, _)) if cur >= mtime => {}
            _ => best = Some((mtime, path)),
        }
    }
    Ok(best.map(|(_, p)| p))
}

fn load_from_file(path: &Path) -> Result<Track> {
    let xml = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let run = livesplit_core::run::parser::livesplit::parse(&xml)
        .map_err(|e| anyhow::anyhow!("parse error: {e}"))?;

    let Some(value) = run.metadata().custom_variable_value(CUSTOM_VAR_NAME) else {
        bail!("no ClassiCubeTrack custom variable");
    };
    let payload = payload::parse(value.as_bytes()).context("parsing ClassiCubeTrack payload")?;

    let labels = labels_from(&run);
    payload::into_track(payload, labels).context("building Track from payload")
}

fn labels_from(run: &Run) -> Vec<String> {
    run.segments().iter().map(|s| s.name().to_owned()).collect()
}
