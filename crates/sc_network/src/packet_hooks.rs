//! Packet send cancellation / observation.
//!
//! Design: one event type with two dispatch paths and a single cancellation API
//! (`SCEvent::set_cancelled/get_cancelled`):
//!
//! - [`PacketSendEvent`]: cancellable; plugins cancel with `set_cancelled(true)`.
//! - 1. Sync dispatch ([`PacketSendHooks`]): `send_packet` awaits each
//!   registered hook ([`PacketSendHook`] is an async trait) before sending.
//!   Plugins can decide `set_cancelled(true)` after awaiting (DB/remote/
//!   arbitrary logic) with no race (packets wait for all hooks).
//!   Zero cost when no hooks are registered.
//! - 2. Async observation (eventbus): `send_sc_event` dispatches on the next tick;
//!   plugins read via `SCEventReader<PacketSendEvent>` (requires
//!   `Events<SCEvent<PacketSendEvent>>`). Cancellation there does not affect
//!   the current send; it is for audit/stats.
//!
//! Cost: packet id is looked up from the concrete type (no payload clone);
//! each registered hook still costs one await (zero when empty).

use async_trait::async_trait;
use parking_lot::RwLock;
use sc_ecs::entity::EntityId;
use sc_ecs::resource::Resource;
use sc_eventbus::events::SCCancellableEvent;
use sc_eventbus::recv::SCEvent;
use std::any::Any;
use std::sync::Arc;

/// Packet send event (cancellable): `set_cancelled(true)` cancels this send.
///
/// Payload carries only packet identity (target/packet id/type name); packet
/// contents are read-only via the `&dyn Any` dispatch argument
/// (`downcast_ref::<T>()` inspects; replacement is unsupported).
#[derive(SCCancellableEvent, Clone, Debug)]
pub struct PacketSendEvent {
    pub entity: EntityId,
    pub packet_id: u16,
    pub packet_name: &'static str,
    pub immediate: bool,
}

/// Packet send hook (async, awaited one by one before `send_packet` sends).
///
/// Two abilities:
/// - Cancel: `event.set_cancelled(true)`;
/// - Inspect: `packet.downcast_ref::<T>()` read-only packet view.
///
/// Because the trait is async, plugins can decide to cancel after `await`
/// (DB/remote checks/arbitrary async logic); packets wait for all hooks.
#[async_trait]
pub trait PacketSendHook: Send + Sync {
    async fn on_send(&self, event: &mut SCEvent<PacketSendEvent>, packet: &(dyn Any + Send + Sync));
}

/// Packet hook registry.
#[derive(Resource, Default)]
pub struct PacketSendHooks {
    hooks: RwLock<Vec<Arc<dyn PacketSendHook>>>,
}

impl PacketSendHooks {
    /// Register a hook.
    pub fn register(&self, hook: Arc<dyn PacketSendHook>) {
        self.hooks.write().push(hook);
    }

    /// Clear all hooks.
    pub fn clear(&self) {
        self.hooks.write().clear();
    }

    /// Whether any hook exists (dispatch is skipped when empty).
    pub fn is_empty(&self) -> bool {
        self.hooks.read().is_empty()
    }

    pub(crate) fn snapshot(&self) -> Vec<Arc<dyn PacketSendHook>> {
        self.hooks.read().clone()
    }

    /// Dispatch: build a cancellable event, await each hook (short-circuit on
    /// cancel), then return whether the send was cancelled.
    ///
    /// - The hook list is snapshotted (Arc clone) before calling to avoid
    ///   register/clear deadlocks;
    /// - The `packet` reference is safe across await (the `send_packet` caller
    ///   holds the packet until the send completes).
    pub async fn dispatch(
        &self,
        entity: EntityId,
        packet_id: u16,
        packet_name: &'static str,
        immediate: bool,
        packet: &(dyn Any + Send + Sync),
    ) -> bool {
        Self::dispatch_snapshot(
            self.snapshot(),
            entity,
            packet_id,
            packet_name,
            immediate,
            packet,
        )
        .await
    }

    pub(crate) async fn dispatch_snapshot(
        hooks: Vec<Arc<dyn PacketSendHook>>,
        entity: EntityId,
        packet_id: u16,
        packet_name: &'static str,
        immediate: bool,
        packet: &(dyn Any + Send + Sync),
    ) -> bool {
        let mut event = SCEvent::new_timestamp(
            entity,
            PacketSendEvent {
                entity,
                packet_id,
                packet_name,
                immediate,
            },
        );
        for hook in hooks.iter() {
            hook.on_send(&mut event, packet).await;
            if event.get_cancelled() {
                break;
            }
        }
        event.get_cancelled()
    }
}
