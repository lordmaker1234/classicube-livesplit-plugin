use tracing::debug;

use crate::plugin::{
    livesplit::{self, Command},
    module::Module,
};

pub struct PauseTriggersModule;

impl PauseTriggersModule {
    pub fn init() -> Self {
        Self
    }
}

impl Module for PauseTriggersModule {
    fn on_new_map(&mut self) {
        debug!("map loading; pausing LiveSplit game time");
        livesplit::send(Command::PauseGameTime);
    }

    fn on_new_map_loaded(&mut self) {
        debug!("map loaded; resuming LiveSplit game time");
        livesplit::send(Command::ResumeGameTime);
    }
}
