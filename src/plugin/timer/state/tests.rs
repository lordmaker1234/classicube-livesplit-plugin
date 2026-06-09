use crate::plugin::{
    livesplit::protocol::{Command, TimingMethod},
    timer::state::{Phase, SplitRow, TimerState},
};

fn start() -> Command {
    Command::Start
}
fn split() -> Command {
    Command::Split
}
fn reset() -> Command {
    Command::Reset { save_attempt: None }
}

/// A minimal fixture: 2 split rows after Start (no track loaded, so split_rows is empty).
/// Clock advances are simulated by passing different `clock` values.

#[test]
fn initial_state_not_running() {
    let state = TimerState::default();
    assert_eq!(state.phase, Phase::NotRunning);
    assert_eq!(state.elapsed_now(100.0), 0.0);
}

#[test]
fn start_begins_running() {
    let mut s = TimerState::default();
    s.apply(&start(), 10.0);
    assert_eq!(s.phase, Phase::Running);
    assert!((s.elapsed_now(12.0) - 2.0).abs() < 1e-9);
}

#[test]
fn elapsed_grows_with_clock() {
    let mut s = TimerState::default();
    s.apply(&start(), 0.0);
    assert!((s.elapsed_now(5.5) - 5.5).abs() < 1e-9);
    assert!((s.elapsed_now(10.0) - 10.0).abs() < 1e-9);
}

#[test]
fn pause_freezes_elapsed() {
    let mut s = TimerState::default();
    s.apply(&start(), 0.0);
    // Run 2 seconds, then pause
    s.apply(&Command::PauseGameTime, 2.0);
    // Clock advances to 5 but elapsed is frozen at 2
    assert!((s.elapsed_now(5.0) - 2.0).abs() < 1e-9);
}

#[test]
fn resume_after_pause_continues_from_freeze_point() {
    let mut s = TimerState::default();
    s.apply(&start(), 0.0);
    s.apply(&Command::PauseGameTime, 2.0); // freeze at 2
    s.apply(&Command::ResumeGameTime, 5.0); // 3 s paused; resume
    // At clock=7: 7 - 0 - 3 paused = 4 running
    assert!((s.elapsed_now(7.0) - 4.0).abs() < 1e-9);
}

#[test]
fn double_pause_is_idempotent() {
    let mut s = TimerState::default();
    s.apply(&start(), 0.0);
    s.apply(&Command::PauseGameTime, 2.0);
    s.apply(&Command::PauseGameTime, 3.0); // second pause is a no-op
    s.apply(&Command::ResumeGameTime, 5.0);
    // 3 s paused (2->5), at clock=7: 7 - 3 = 4
    assert!((s.elapsed_now(7.0) - 4.0).abs() < 1e-9);
}

#[test]
fn double_resume_is_idempotent() {
    let mut s = TimerState::default();
    s.apply(&start(), 0.0);
    s.apply(&Command::PauseGameTime, 2.0);
    s.apply(&Command::ResumeGameTime, 4.0); // 2 s paused
    s.apply(&Command::ResumeGameTime, 5.0); // no-op
    assert!((s.elapsed_now(6.0) - 4.0).abs() < 1e-9); // 6 - 0 - 2 = 4
}

#[test]
fn reset_returns_to_not_running() {
    let mut s = TimerState::default();
    s.apply(&start(), 0.0);
    s.apply(&reset(), 5.0);
    assert_eq!(s.phase, Phase::NotRunning);
    assert_eq!(s.elapsed_now(10.0), 0.0);
}

#[test]
fn split_or_start_from_not_running_starts() {
    let mut s = TimerState::default();
    s.apply(&Command::SplitOrStart, 5.0);
    assert_eq!(s.phase, Phase::Running);
    assert!((s.elapsed_now(7.0) - 2.0).abs() < 1e-9);
}

#[test]
fn initialize_game_time_sets_flag() {
    let mut s = TimerState::default();
    s.apply(&start(), 0.0);
    assert!(!s.game_time_initialized);
    s.apply(&Command::InitializeGameTime, 0.0);
    assert!(s.game_time_initialized);
}

#[test]
fn set_current_timing_method_is_noop() {
    let mut s = TimerState::default();
    s.apply(
        &Command::SetCurrentTimingMethod {
            timing_method: TimingMethod::GameTime,
        },
        0.0,
    );
    assert_eq!(s.phase, Phase::NotRunning);
}

// ---- Tests with synthetic split rows (injected directly, bypassing current_track()) ----

fn state_with_rows(n: usize) -> TimerState {
    use crate::plugin::splits::geometry::CheckpointKind;
    let mut s = TimerState::default();
    s.phase = Phase::Running;
    s.start_at = Some(0.0);
    s.split_rows = (0..n)
        .map(|i| SplitRow {
            kind: if i + 1 == n {
                CheckpointKind::End
            } else {
                CheckpointKind::Split
            },
            label: format!("cp{i}"),
            time: None,
        })
        .collect();
    s
}

#[test]
fn split_captures_time_and_advances_cursor() {
    let mut s = state_with_rows(3);
    s.apply(&split(), 2.0);
    assert_eq!(s.cursor, 1);
    assert!((s.split_rows[0].time.unwrap() - 2.0).abs() < 1e-9);
    assert_eq!(s.phase, Phase::Running);
}

#[test]
fn final_split_ends_run() {
    let mut s = state_with_rows(3);
    s.apply(&split(), 2.0);
    s.apply(&split(), 4.0);
    s.apply(&split(), 6.0);
    assert_eq!(s.phase, Phase::Ended);
    assert!((s.frozen_final.unwrap() - 6.0).abs() < 1e-9);
    // Elapsed stays frozen
    assert!((s.elapsed_now(100.0) - 6.0).abs() < 1e-9);
}

#[test]
fn split_no_track_is_noop_clock_runs_forever() {
    let mut s = TimerState::default();
    s.apply(&start(), 0.0); // no track -> split_rows empty
    s.apply(&split(), 5.0);
    assert_eq!(s.phase, Phase::Running);
    assert_eq!(s.cursor, 0);
    assert!((s.elapsed_now(10.0) - 10.0).abs() < 1e-9);
}

#[test]
fn undo_split_walks_back() {
    let mut s = state_with_rows(3);
    s.apply(&split(), 2.0);
    s.apply(&split(), 4.0);
    s.apply(&Command::UndoSplit, 4.5);
    assert_eq!(s.cursor, 1);
    assert!(s.split_rows[1].time.is_none());
    assert_eq!(s.phase, Phase::Running);
}

#[test]
fn undo_split_from_ended_resumes_running() {
    let mut s = state_with_rows(2);
    s.apply(&split(), 2.0);
    s.apply(&split(), 4.0);
    assert_eq!(s.phase, Phase::Ended);
    s.apply(&Command::UndoSplit, 4.5);
    assert_eq!(s.phase, Phase::Running);
    assert!(s.frozen_final.is_none());
    assert_eq!(s.cursor, 1);
}

#[test]
fn undo_at_cursor_zero_is_noop() {
    let mut s = state_with_rows(2);
    s.apply(&Command::UndoSplit, 1.0);
    assert_eq!(s.cursor, 0);
    assert_eq!(s.phase, Phase::Running);
}

#[test]
fn skip_split_advances_without_capturing() {
    let mut s = state_with_rows(3);
    s.apply(&Command::SkipSplit, 2.0);
    assert_eq!(s.cursor, 1);
    assert!(s.split_rows[0].time.is_none());
    assert_eq!(s.phase, Phase::Running);
}

#[test]
fn skip_last_split_is_noop() {
    let mut s = state_with_rows(2);
    s.apply(&split(), 2.0); // cursor -> 1 (last)
    s.apply(&Command::SkipSplit, 3.0); // last split: no-op
    assert_eq!(s.cursor, 1);
    assert_eq!(s.phase, Phase::Running);
}

#[test]
fn split_or_start_from_running_splits() {
    let mut s = state_with_rows(3);
    s.apply(&Command::SplitOrStart, 2.0);
    assert_eq!(s.cursor, 1);
    assert!(s.split_rows[0].time.is_some());
}
