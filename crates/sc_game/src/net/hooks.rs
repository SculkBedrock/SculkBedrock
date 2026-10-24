//! Intent 拦截钩子（游戏层，在翻译前 await）。
//!
//! Intent interception hooks (game-layer, awaited before translation).
//!
//! 召回语义与 `sc_network::packet_hooks` 相同；`intent` 是只读的。
//! 插件可以在 await 之后决定召回，无竞争问题。
//! Same recall semantics as `sc_network::packet_hooks`; `intent` is
//! read-only. Plugins may decide to recall after awaiting without races.

use super::intent::NetworkIntent;
use async_trait::async_trait;
use parking_lot::RwLock;
use sc_ecs::resource::Resource;
use std::sync::Arc;

/// Intent 发送事件（可召回）：取消即撤回该 intent。
///
/// Intent send event (recallable): cancelling withdraws the intent.
///
/// 广播/世界级 intent 没有 ECS 实体目标：它们自带召回标志、自包含，
/// 形状与 packet_hooks 事件相同。
/// Broadcast/world-level intents carry no ECS entity target: they are
/// self-contained with a recall flag, shaped like packet_hooks events.
#[derive(Clone, Debug)]
pub struct IntentSendEvent {
    /// 目标实体运行时 id（无目标时为 None）。
    ///
    /// Target entity runtime id (None without a target).
    pub entity_id: Option<u64>,
    /// intent 变体名。
    ///
    /// Intent variant name.
    pub intent_kind: &'static str,
    /// 召回标志：任一钩子置位即撤回。
    ///
    /// Recall flag: set by any hook to withdraw the intent.
    cancelled: bool,
}

impl IntentSendEvent {
    /// 读取召回标志。
    ///
    /// Reads the recall flag.
    pub fn get_cancelled(&self) -> bool {
        self.cancelled
    }

    /// 设置召回标志。
    ///
    /// Sets the recall flag.
    ///
    /// - `cancelled`: 是否召回（撤回该 intent）
    pub fn set_cancelled(&mut self, cancelled: bool) {
        self.cancelled = cancelled;
    }
}

/// Intent 拦截钩子（异步，在 outbox 翻译前 await）。
///
/// Intent interception hooks (async, awaited before outbox translation).
#[async_trait]
pub trait IntentSendHook: Send + Sync {
    /// 处理一个待发送 intent；可通过事件置位召回标志来撤回。
    ///
    /// Handles one outbound intent; recall by setting the event flag.
    ///
    /// - `event`: 发送事件（含目标与召回标志）
    /// - `intent`: 待发送的 intent（只读）
    async fn on_intent(&self, event: &mut IntentSendEvent, intent: &NetworkIntent);
}

/// Intent 拦截注册表（由 sc_game 插入的资源）。
///
/// Intent interception registry (resource inserted by sc_game).
#[derive(Resource, Default)]
pub struct IntentSendHooks {
    hooks: RwLock<Vec<Arc<dyn IntentSendHook>>>,
}

impl IntentSendHooks {
    /// 注册一个钩子。
    ///
    /// Registers a hook.
    ///
    /// - `hook`: 要注册的钩子
    pub fn register(&self, hook: Arc<dyn IntentSendHook>) {
        self.hooks.write().push(hook);
    }

    /// 清空所有钩子。
    ///
    /// Clears all hooks.
    pub fn clear(&self) {
        self.hooks.write().clear();
    }

    /// 是否没有注册任何钩子。
    ///
    /// Whether no hook is registered.
    pub fn is_empty(&self) -> bool {
        self.hooks.read().is_empty()
    }

    /// 快照钩子列表（持有 Arc，await 期间不持锁）。
    ///
    /// Snapshot the hook list (owned Arc, no locks across await).
    pub fn snapshot(&self) -> Vec<Arc<dyn IntentSendHook>> {
        self.hooks.read().clone()
    }
}

/// 对快照逐个 await 钩子：任一召回即短路。
/// 运行期间不持有资源守卫（可安全跨 await）。
///
/// Awaits hooks from a snapshot: any recall short-circuits.
/// Runs without holding resource guards across await.
///
/// - `snapshot`: 钩子快照
/// - `intent`: 待发送的 intent
///
/// # Returns
/// 任一钩子召回返回 `true`，否则 `false`。
///
/// Returns `true` if any hook recalled the intent.
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
