//! In-world HUD overlay for the loaded track's checkpoint volumes.
//!
//! Three layers, all driven off this module's single tick and the same
//! map-scoped `splits::visible_aabbs()` snapshot:
//!
//! 1. [`boxes`] -- each `Trigger::Aabb` checkpoint as a translucent colored
//!    cuboid via ClassiCube's built-in selection `CC_API`, color-coded by
//!    `CheckpointKind`. The engine re-renders its selection list every
//!    frame, so this layer never touches the GPU -- it's pure state
//!    management.
//! 2. [`labels`] -- each checkpoint's `label` string as white drop-shadowed
//!    text, drawn as a camera-facing billboard floating above its box. This
//!    layer touches the GPU: per-label text textures and a dynamic VB, with
//!    context-lost/recreated handling.
//! 3. [`lines`] -- a colored camera-facing ribbon connecting consecutive
//!    label anchors, drawn via a `VERTEX_FORMAT_COLOURED` VB.
//!
//! All three layers are `Module` children of [`HudModule`], so teardown and
//! the map-change / reset invalidation flow through the recursive dispatch.
//! `HudModule` itself owns the shared tick, the single `OwnedScreen` render
//! hook (registered at `HUD - 2` by [`render::install`]), and the
//! `/client LiveSplit show [on|off]` toggle (default on, in-memory only).
//! The render hook runs the lines pass first, then labels, so route ribbons
//! draw underneath the floating text. The per-tick `visible` snapshot is
//! fanned out to all three layers' `reconcile` functions.

mod boxes;
mod labels;
mod lines;
mod render;
mod shared;

use std::cell::Cell;

use classicube_helpers::tick::TickEventHandler;
use classicube_sys::OwnedScreen;

use self::{boxes::BoxesModule, labels::LabelsModule, lines::LinesModule};
use crate::plugin::{editor, module::Module, splits};

/// Private selection-id range for plugin-owned checkpoint boxes. Server
/// commands (`/zone`, `/measure`) allocate selection ids from 0 upward,
/// so a high base keeps us from clobbering them (and vice versa).
/// `200..=254` gives 55 simultaneous boxes; the engine's `SELECTIONS_MAX`
/// is 256. Id 255 is reserved for the editor's rubber-band preview
/// (`editor::preview::PREVIEW_SELECTION_ID`). The label layer honors the
/// same cap so the two stay in lockstep.
const HUD_ID_BASE: u8 = 200;
/// Highest id (inclusive) for HUD checkpoint boxes. Id 255 is reserved for
/// the editor's rubber-band preview.
const HUD_ID_LAST: u8 = 254;
/// Number of ids in `HUD_ID_BASE..=HUD_ID_LAST` (= 55).
const HUD_ID_COUNT: usize = (HUD_ID_LAST - HUD_ID_BASE) as usize + 1;

thread_local! {
    /// Whether the HUD is currently showing. Set by the
    /// `/client LiveSplit show` chat arm; read by the tick reconcile.
    static SHOW: Cell<bool> = const { Cell::new(true) };
}

pub fn set_show(show: bool) {
    SHOW.set(show);
}

/// Flip the HUD visibility and return the new state.
pub fn toggle_show() -> bool {
    let show = !SHOW.get();
    SHOW.set(show);
    show
}

pub struct HudModule {
    boxes: BoxesModule,
    labels: LabelsModule,
    lines: LinesModule,
    // Owned for its Drop side-effect: TickEventHandler::Drop unregisters
    // the reconcile closure from the helpers crate's tick callback list.
    _tick: TickEventHandler,
    // The single shared render hook (both line and label passes). Declared
    // last so the tick unsubscribes before the screen unregisters on struct
    // drop (idempotent either way, but matches the handle-drop convention).
    _screen: OwnedScreen,
}

impl HudModule {
    pub fn init() -> Self {
        // In-memory only: every Init starts with the HUD on.
        SHOW.set(true);

        let boxes = BoxesModule::init();
        let labels = LabelsModule::init();
        let lines = LinesModule::init();

        let mut tick = TickEventHandler::new();
        tick.on(|_event| reconcile());

        let screen = render::install();

        Self {
            boxes,
            labels,
            lines,
            _tick: tick,
            _screen: screen,
        }
    }
}

impl Module for HudModule {
    fn children(&mut self) -> Vec<&mut dyn Module> {
        vec![&mut self.boxes, &mut self.labels, &mut self.lines]
    }
}

/// Fetch the map-scoped checkpoint set once and hand it to both layers.
/// While the HUD is hidden the set is empty, so both layers reconcile down
/// to nothing. Sharing one fetch keeps the two layers on the same snapshot
/// within a frame.
fn reconcile() {
    let mut visible = if SHOW.get() {
        splits::visible_aabbs()
    } else {
        Vec::new()
    };

    // Authoring view: there's no live run to target, so drop the
    // next-checkpoint highlight (the brighter box + the `> <` label marker).
    // Without this the idle cursor (next_index == 0) would keep flagging
    // Start, and the always-present edit-mode label body means the marker
    // would never self-suppress. Both child diff keys track this bool, so
    // clearing it re-renders them on the edit-mode toggle.
    if editor::is_enabled() {
        for entry in &mut visible {
            entry.4 = false;
        }
    }

    boxes::reconcile(&visible);
    labels::reconcile(&visible);
    lines::reconcile(&visible);
}
