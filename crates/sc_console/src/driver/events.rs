//! Console driver: per-frame event drain and boot-skip helpers (split from `driver.rs`; re-exported at `crate::driver`).

use super::*;

/// Per-frame event scan result (unit-testable).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct EventBatch {
    /// Key events seen this frame, in order.
    pub keys: Vec<KeyEvent>,
    /// Non-key events seen this frame (resize/mouse/focus), counted only.
    pub other: usize,
}

/// Main-loop per-frame event cap: drains mixed key/mouse/resize input one by one, deferring overflow.
///
/// 64 events per ~16ms frame is far above human input; window-drag resize floods or
/// wheel bursts only slip by about one frame (~16-50ms), so keys are never buried.
pub(crate) const MAIN_DRAIN_MAX: usize = 64;

/// Per-frame event drain cap: keeps mouse floods from starving rendering.
///
/// 4096 covers ~240k events/s, far above real mouse drags (~100Hz); on overflow the frame drops
/// the rest and continues next frame (keys slip by about one frame, ~16ms).
pub(crate) const EVENT_DRAIN_MAX: usize = 4096;

/// Drain the ready events for one frame (`next` returns `None` when the queue is empty).
///
/// With mouse capture on, mouse moves keep producing `Event::Mouse`. Reading only one per frame
/// drops input and buries keys behind the flood, delaying them by seconds.
/// Drain the whole frame here (bounded by `EVENT_DRAIN_MAX`); the caller keeps
/// key detail (code plus modifiers) for stage handling.
///
/// Pure logic (the `next` event closure is injected) for unit tests.
pub(crate) fn drain_events(mut next: impl FnMut() -> Option<Event>) -> EventBatch {
    let mut batch = EventBatch {
        keys: Vec::new(),
        other: 0,
    };
    while batch.keys.len() + batch.other < EVENT_DRAIN_MAX {
        match next() {
            Some(Event::Key(k)) => batch.keys.push(k),
            Some(_) => batch.other += 1,
            None => break,
        }
    }
    batch
}

/// Skip the boot animation on any key (jump to the end state); returns whether it was playing.
///
/// Input stays locked during the welcome exit (600ms) plus boot animation (1100ms);
/// the combined wait can be skipped with one keypress.
pub(super) fn skip_boot_anim(state: &mut GuiState) -> bool {
    state.boot_t0.take().is_some()
}
