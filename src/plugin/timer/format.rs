#[cfg(test)]
mod tests;

/// Format a non-negative elapsed time (in seconds) as `M:SS.mmm` (below 1 h)
/// or `H:MM:SS.mmm` (1 h and above). Negative input is clamped to 0.
///
/// Uses integer millisecond arithmetic to avoid floating-point rounding drift.
pub fn format_time(secs: f64) -> String {
    let ms = (secs.max(0.0) * 1000.0).round() as u64;
    let total_secs = ms / 1000;
    let millis = ms % 1000;
    let total_mins = total_secs / 60;
    let seconds = total_secs % 60;
    let hours = total_mins / 60;
    let minutes = total_mins % 60;

    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}.{millis:03}")
    } else {
        format!("{minutes}:{seconds:02}.{millis:03}")
    }
}
