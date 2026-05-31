use tracing::debug;

use crate::plugin::{
    livesplit::{self, Command},
    module::Module,
    splits,
};

pub struct PauseTriggersModule;

impl PauseTriggersModule {
    pub fn init() -> Self {
        Self
    }
}

impl Module for PauseTriggersModule {
    fn on_new_map(&mut self) {
        // Without a loaded track there's no run to game-time, and LSO
        // would respond `NoRunInProgress` which surfaces as a chat error.
        if !splits::track_loaded() {
            return;
        }
        debug!("map loading; pausing LiveSplit game time");
        livesplit::send(Command::PauseGameTime);
    }

    fn on_new_map_loaded(&mut self) {
        if !splits::track_loaded() {
            return;
        }
        debug!("map loaded; resuming LiveSplit game time");
        livesplit::send(Command::ResumeGameTime);
    }
}
