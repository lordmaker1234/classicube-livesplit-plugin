//! Built-in self-contained timer: subscribes to the `CMD_TX` broadcast
//! (the same stream the LSO server and Windows named-pipe client receive)
//! and maintains a minimal game-time timer state machine + a 2D screen-space
//! overlay showing the running clock and per-split times.
//!
//! The LiveSplit IPC is kept intact: the built-in timer is purely additive.
//! When the module is active, `livesplit::any_connected()` returns `true` even
//! with no external app connected, making the plugin fully self-contained.

mod context;
mod format;
mod render;
mod state;
mod texture;

use std::{
    cell::{Cell, RefCell},
    time::Instant,
};

use classicube_helpers::events::gfx::{ContextLostEventHandler, ContextRecreatedEventHandler};
use classicube_sys::OwnedScreen;
use tracing::debug;

pub use self::state::TimerState;
use crate::plugin::{livesplit::protocol::Command, module::Module};

thread_local! {
    /// The live timer state, applied on the main thread from the CMD_TX
    /// forwarder and read by the render hook each frame.
    pub(crate) static TIMER_STATE: RefCell<Option<TimerState>> = const { RefCell::new(None) };

    /// Whether the timer overlay is currently visible. Toggled by
    /// `/client LiveSplit timer [on|off]`.
    static SHOW: Cell<bool> = const { Cell::new(true) };

    /// Monotonic origin captured once at `TimerModule::init`. All clock()
    /// readings are relative to this, giving a stable f64 second counter.
    static CLOCK_ORIGIN: RefCell<Option<Instant>> = const { RefCell::new(None) };
}

/// Current monotonic clock value in seconds, relative to the module origin.
pub(crate) fn clock() -> f64 {
    CLOCK_ORIGIN.with_borrow(|o| {
        o.as_ref()
            .map(|origin| origin.elapsed().as_secs_f64())
            .unwrap_or(0.0)
    })
}

/// Apply one command to the main-thread timer state. Called by the forwarder
/// task (hopped via `spawn_on_main_thread`). No-ops if the module is torn down.
pub fn apply_command(cmd: Command) {
    if !SHOW.get() {
        // Still apply to state even when display is off — the run advances.
    }
    TIMER_STATE.with_borrow_mut(|slot| {
        if let Some(state) = slot.as_mut() {
            state.apply(&cmd, clock());
        }
    });
}

pub fn set_show(show: bool) {
    SHOW.set(show);
}

pub fn toggle_show() -> bool {
    let show = !SHOW.get();
    SHOW.set(show);
    show
}

/// Drop cached render textures (called by `context::context_lost` and by
/// `TimerModule::reset`/`free`).
pub(crate) fn invalidate_cache() {
    render::invalidate();
}

pub struct TimerModule {
    _screen: OwnedScreen,
    _context_lost: ContextLostEventHandler,
    _context_recreated: ContextRecreatedEventHandler,
}

impl TimerModule {
    pub fn init() -> Self {
        CLOCK_ORIGIN.with_borrow_mut(|o| *o = Some(Instant::now()));
        TIMER_STATE.with_borrow_mut(|s| *s = Some(TimerState::default()));
        SHOW.set(true);

        let (context_lost, context_recreated) = context::subscribe();
        let screen = render::install();

        debug!("built-in timer initialized");
        Self {
            _screen: screen,
            _context_lost: context_lost,
            _context_recreated: context_recreated,
        }
    }
}

impl Module for TimerModule {
    fn free(&mut self) {
        context::drop_buffer();
        invalidate_cache();
        texture::free();
        TIMER_STATE.with_borrow_mut(|s| *s = None);
        CLOCK_ORIGIN.with_borrow_mut(|o| *o = None);
        debug!("built-in timer freed");
    }

    fn reset(&mut self) {
        // ClassiCube Reset (disconnect / local-map-load): zero the run display.
        // Keep the screen, VB, and font live (like HudModule).
        TIMER_STATE.with_borrow_mut(|s| {
            if let Some(state) = s.as_mut() {
                *state = TimerState::default();
            }
        });
        invalidate_cache();
    }
}
