#[cfg(test)]
mod tests;

/// Format a non-negative elapsed time (in seconds) as `M:SS.cc` (below 1 h)
/// or `H:MM:SS.cc` (1 h and above). Negative input is clamped to 0.
///
/// Uses integer centisecond arithmetic to avoid floating-point rounding drift.
pub fn format_time(secs: f64) -> String {
    let cs = (secs.max(0.0) * 100.0).round() as u64;
    let total_secs = cs / 100;
    let centis = cs % 100;
    let total_mins = total_secs / 60;
    let seconds = total_secs % 60;
    let hours = total_mins / 60;
    let minutes = total_mins % 60;

    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}.{centis:02}")
    } else {
        format!("{minutes}:{seconds:02}.{centis:02}")
    }
}
