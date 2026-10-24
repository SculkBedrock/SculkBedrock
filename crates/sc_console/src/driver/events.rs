//! 控制台驱动：逐帧事件抽取与开场跳过辅助（由 `driver.rs` 拆出；在 `crate::driver` 重新导出）。
//!
//! Console driver: per-frame event extraction and boot-skip helpers
//! (split out of `driver.rs`; re-exported from `crate::driver`).

use super::*;

/// 每帧事件扫描结果。
///
/// Per-frame event scan result.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct EventBatch {
    /// Key events seen this frame, in order.
    pub keys: Vec<KeyEvent>,
    /// Non-key events seen this frame (resize/mouse/focus), counted only.
    pub other: usize,
}

/// Main-loop per-frame event cap: drains mixed key/mouse/resize input one by one, deferring overflow.
pub(crate) const MAIN_DRAIN_MAX: usize = 64;

/// Per-frame event drain cap: keeps mouse floods from starving rendering.
/// On overflow the frame drops the rest and continues next frame.
pub(crate) const EVENT_DRAIN_MAX: usize = 4096;

/// Drain the ready events for one frame (`next` returns `None` when the queue is empty).
/// Drains at most `EVENT_DRAIN_MAX` events; the caller keeps
/// key detail (code plus modifiers) for stage handling.
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
