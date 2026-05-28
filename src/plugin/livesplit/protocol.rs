use std::time::Duration;

use serde::{Serialize, Serializer};

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "command", rename_all = "camelCase")]
pub enum Command {
    Start,
    Split,
    SplitOrStart,
    #[serde(rename_all = "camelCase")]
    Reset {
        #[serde(skip_serializing_if = "Option::is_none")]
        save_attempt: Option<bool>,
    },
    Pause,
    Resume,
    UndoSplit,
    SkipSplit,
    InitializeGameTime,
    SetGameTime {
        #[serde(serialize_with = "TimeSpan::serialize")]
        time: TimeSpan,
    },
    PauseGameTime,
    ResumeGameTime,
    SetLoadingTimes {
        #[serde(serialize_with = "TimeSpan::serialize")]
        time: TimeSpan,
    },
    Ping,
}

/// A duration serialized as `<secs>.<9-digit-nanos>` to match
/// livesplit-core's wire format (see `serialize_time_span` in
/// livesplit-core/src/networking/server_protocol.rs:103-109).
#[derive(Clone, Copy, Debug)]
pub struct TimeSpan(pub Duration);

impl From<Duration> for TimeSpan {
    fn from(d: Duration) -> Self {
        Self(d)
    }
}

impl TimeSpan {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let secs = self.0.as_secs();
        let nanos = self.0.subsec_nanos();
        s.collect_str(&format_args!("{secs}.{nanos:09}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(c: &Command) -> String {
        serde_json::to_string(c).unwrap()
    }

    #[test]
    fn unit_variants_serialize_camel_case() {
        assert_eq!(json(&Command::Start), r#"{"command":"start"}"#);
        assert_eq!(json(&Command::Split), r#"{"command":"split"}"#);
        assert_eq!(
            json(&Command::SplitOrStart),
            r#"{"command":"splitOrStart"}"#
        );
        assert_eq!(json(&Command::Ping), r#"{"command":"ping"}"#);
        assert_eq!(
            json(&Command::InitializeGameTime),
            r#"{"command":"initializeGameTime"}"#
        );
    }

    #[test]
    fn reset_without_save_attempt_omits_field() {
        assert_eq!(
            json(&Command::Reset { save_attempt: None }),
            r#"{"command":"reset"}"#
        );
    }

    #[test]
    fn reset_with_save_attempt_uses_camel_case_key() {
        assert_eq!(
            json(&Command::Reset {
                save_attempt: Some(true)
            }),
            r#"{"command":"reset","saveAttempt":true}"#
        );
    }

    #[test]
    fn set_game_time_serializes_secs_and_padded_nanos() {
        let cmd = Command::SetGameTime {
            time: TimeSpan(Duration::new(83, 25)),
        };
        assert_eq!(
            json(&cmd),
            r#"{"command":"setGameTime","time":"83.000000025"}"#
        );
    }

    #[test]
    fn set_loading_times_zero_pads_to_nine_digits() {
        let cmd = Command::SetLoadingTimes {
            time: TimeSpan(Duration::from_millis(1500)),
        };
        assert_eq!(
            json(&cmd),
            r#"{"command":"setLoadingTimes","time":"1.500000000"}"#
        );
    }
}
