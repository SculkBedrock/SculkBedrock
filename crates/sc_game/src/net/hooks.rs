//! Intent interception hooks (game-layer, awaited before translation).
//!
//! Same recall semantics as `sc_network::packet_hooks`; `intent` is
//! read-only. Plugins may decide to recall after awaiting without races.

use super::intent::NetworkIntent;
use async_trait::async_trait;
use parking_lot::RwLock;
use sc_ecs::resource::Resource;
use std::sync::Arc;

/// Intent send event (recallable): cancelling withdraws the intent.
///
/// Broadcast/world-level intents carry no ECS entity target: they are
/// self-contained with a recall flag, shaped like packet_hooks events.
#[derive(Clone, Debug)]
pub struct IntentSendEvent {
    /// Target entity runtime id (None without a target).
    pub entity_id: Option<u64>,
    /// Intent variant name.
    pub intent_kind: &'static str,
    cancelled: bool,
}

impl IntentSendEvent {
    pub fn get_cancelled(&self) -> bool {
        self.cancelled
    }

    pub fn set_cancelled(&mut self, cancelled: bool) {
        self.cancelled = cancelled;
    }
}

/// Intent interception hooks (async, awaited before outbox translation).
#[async_trait]
pub trait IntentSendHook: Send + Sync {
    async fn on_intent(&self, event: &mut IntentSendEvent, intent: &NetworkIntent);
}

/// Intent interception registry (resource inserted by sc_game).
#[derive(Resource, Default)]
pub struct IntentSendHooks {
    hooks: RwLock<Vec<Arc<dyn IntentSendHook>>>,
}

impl IntentSendHooks {
    pub fn register(&self, hook: Arc<dyn IntentSendHook>) {
        self.hooks.write().push(hook);
    }

    pub fn clear(&self) {
        self.hooks.write().clear();
    }

    pub fn is_empty(&self) -> bool {
        self.hooks.read().is_empty()
    }

    /// Snapshot the hook list (owned Arc, no locks across await).
    pub fn snapshot(&self) -> Vec<Arc<dyn IntentSendHook>> {
        self.hooks.read().clone()
    }
}

/// Await hooks from a snapshot: any recall short-circuits.
/// Runs without holding resource guards across await.
pub async fn dispatch_intent_hooks(
    snapshot: &[Arc<dyn IntentSendHook>],
    intent: &NetworkIntent,
) -> bool {
    let mut event = IntentSendEvent {
        entity_id: intent.entity_id(),
        intent_kind: intent.kind(),
        cancelled: false,
    };
    for hook in snapshot {
        hook.on_intent(&mut event, intent).await;
        if event.get_cancelled() {
            break;
        }
    }
    event.get_cancelled()
}
