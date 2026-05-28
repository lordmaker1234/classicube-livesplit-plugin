use classicube_helpers::async_manager;

use crate::plugin::module::Module;

pub struct AsyncManagerModule;

impl AsyncManagerModule {
    pub fn init() -> Self {
        async_manager::initialize();
        Self
    }
}

impl Module for AsyncManagerModule {
    fn free(&mut self) {
        async_manager::shutdown();
    }
}
