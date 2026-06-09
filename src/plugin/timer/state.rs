#[cfg(test)]
mod tests;

use crate::plugin::{livesplit::protocol::Command, splits, splits::geometry::CheckpointKind};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    NotRunning,
    Running,
    Ended,
}

/// One row shown in the in-game timer overlay, corresponding to one
/// `Command::Split` the geometry layer fired.
#[derive(Debug, Clone, PartialEq)]
pub struct SplitRow {
    pub kind: CheckpointKind,
    pub label: String,
    /// Captured game-time at the moment this split fired, or `None` if not yet reached.
    pub time: Option<f64>,
}

/// Minimal game-time timer state driven by the same `Command` stream the
/// geometry layer broadcasts. All timing is in seconds as `f64`. The `clock`
/// parameter passed to `apply` and `elapsed_now` must come from the same
/// monotonic clock source (`timer::clock()`) so the two paths agree.
#[derive(Debug, Clone)]
pub struct TimerState {
    pub phase: Phase,
    /// Monotonic clock value captured at `Command::Start`.
    start_at: Option<f64>,
    pub game_time_initialized: bool,
    /// True while `PauseGameTime` is active (i.e. the pause counter is > 0).
    paused: bool,
    /// Total seconds already accumulated from completed pauses.
    paused_accum: f64,
    /// Clock value when the current pause began.
    pause_started_at: Option<f64>,
    /// Per-segment rows, populated at `Start` from the loaded track's
    /// non-Start checkpoints. `split_rows[i].time` is filled when the
    /// i-th `Command::Split` arrives.
    pub split_rows: Vec<SplitRow>,
    /// Index of the next expected `Command::Split` (0-based into `split_rows`).
    pub cursor: usize,
    /// Frozen final game-time captured when the last split fires (phase -> Ended).
    pub frozen_final: Option<f64>,
}

impl Default for TimerState {
    fn default() -> Self {
        Self {
            phase: Phase::NotRunning,
            start_at: None,
            game_time_initialized: false,
            paused: false,
            paused_accum: 0.0,
            pause_started_at: None,
            split_rows: Vec::new(),
            cursor: 0,
            frozen_final: None,
        }
    }
}

impl TimerState {
    /// Current displayed game time in seconds.
    ///
    /// - `NotRunning`: always 0.
    /// - `Ended`: the stable frozen value from when the last split fired.
    /// - `Running`: wall elapsed minus total paused time; freezes while paused.
    pub fn elapsed_now(&self, clock: f64) -> f64 {
        match self.phase {
            Phase::NotRunning => 0.0,
            Phase::Ended => self.frozen_final.unwrap_or(0.0),
            Phase::Running => {
                let Some(start_at) = self.start_at else {
                    return 0.0;
                };
                let live_pause = if self.paused {
                    self.pause_started_at.map_or(0.0, |p| clock - p)
                } else {
                    0.0
                };
                (clock - start_at - self.paused_accum - live_pause).max(0.0)
            }
        }
    }

    /// Apply one command to the timer state. `clock` is the current monotonic
    /// clock value (seconds); pass a fixed value in tests.
    pub fn apply(&mut self, cmd: &Command, clock: f64) {
        match cmd {
            Command::Start => self.do_start(clock),
            Command::SplitOrStart => match self.phase {
                Phase::NotRunning => self.do_start(clock),
                Phase::Running | Phase::Ended => self.do_split(clock),
            },
            Command::InitializeGameTime => {
                self.game_time_initialized = true;
            }
            Command::Split => self.do_split(clock),
            Command::Reset { .. } => *self = Self::default(),
            Command::PauseGameTime => {
                if self.phase == Phase::Running && !self.paused {
                    self.paused = true;
                    self.pause_started_at = Some(clock);
                }
            }
            Command::ResumeGameTime => {
                if self.paused {
                    let delta = self.pause_started_at.map_or(0.0, |p| clock - p);
                    self.paused_accum += delta;
                    self.paused = false;
                    self.pause_started_at = None;
                }
            }
            Command::UndoSplit => {
                if self.cursor > 0 {
                    if self.phase == Phase::Ended {
                        self.phase = Phase::Running;
                        self.frozen_final = None;
                    }
                    self.cursor -= 1;
                    if self.cursor < self.split_rows.len() {
                        self.split_rows[self.cursor].time = None;
                    }
                }
            }
            Command::SkipSplit => {
                if self.phase == Phase::Running
                    && !self.split_rows.is_empty()
                    && self.cursor + 1 < self.split_rows.len()
                {
                    self.split_rows[self.cursor].time = None;
                    self.cursor += 1;
                }
            }
            // RTA pause/resume and keepalive: not part of game-time display.
            Command::Pause | Command::Resume | Command::Ping => {}
            // Never sent today by this plugin.
            Command::SetGameTime { .. } | Command::SetLoadingTimes { .. } => {}
            Command::SetCurrentTimingMethod { .. } => {}
        }
    }

    fn do_start(&mut self, clock: f64) {
        self.phase = Phase::Running;
        self.start_at = Some(clock);
        self.game_time_initialized = false;
        self.paused = false;
        self.paused_accum = 0.0;
        self.pause_started_at = None;
        self.cursor = 0;
        self.frozen_final = None;
        // One row per Command::Split the geometry layer will send: checkpoints[1..]
        // (checkpoint[0] is Start, which fires Command::Start, not Command::Split).
        // If no track is loaded, split_rows stays empty and the clock runs forever.
        let track = splits::current_track();
        self.split_rows = track
            .as_ref()
            .map(|t| {
                t.checkpoints[1..]
                    .iter()
                    .map(|cp| SplitRow {
                        kind: cp.kind,
                        label: cp.label.clone(),
                        time: None,
                    })
                    .collect()
            })
            .unwrap_or_default();
    }

    fn do_split(&mut self, clock: f64) {
        if self.phase != Phase::Running {
            return;
        }
        if self.split_rows.is_empty() {
            // No track loaded: splits are no-ops; the clock runs on.
            return;
        }
        if self.cursor < self.split_rows.len() {
            let t = self.elapsed_now(clock);
            self.split_rows[self.cursor].time = Some(t);
            self.cursor += 1;
            if self.cursor >= self.split_rows.len() {
                self.frozen_final = Some(t);
                self.phase = Phase::Ended;
            }
        }
    }
}
