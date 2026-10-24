//! 数据包发送的取消与观测。
//!
//! 一个事件类型 [`PacketSendEvent`] 配两条分发路径，取消 API 为
//! `SCEvent::set_cancelled/get_cancelled`：
//!
//! - 同步分发 [`PacketSendHooks`]：`send_packet` 在发送前逐个 await 已注册的
//!   [`PacketSendHook`]（异步 trait）。未注册钩子时不产生开销。
//! - 异步观测（eventbus）：`send_sc_event` 在下一 tick 分发，插件通过
//!   `SCEventReader<PacketSendEvent>` 读取；此处的取消不影响当前发送。
//!
//! Packet send cancellation / observation.
//!
//! One event type [`PacketSendEvent`] with two dispatch paths and a single
//! cancellation API (`SCEvent::set_cancelled/get_cancelled`):
//!
//! - Sync dispatch ([`PacketSendHooks`]): `send_packet` awaits each registered
//!   hook ([`PacketSendHook`] is an async trait) before sending. Zero cost when
//!   no hooks are registered.
//! - Async observation (eventbus): `send_sc_event` dispatches on the next tick;
//!   plugins read via `SCEventReader<PacketSendEvent>`. Cancellation there does
//!   not affect the current send.

use async_trait::async_trait;
use parking_lot::RwLock;
use sc_ecs::entity::EntityId;
use sc_ecs::resource::Resource;
use sc_eventbus::events::SCCancellableEvent;
use sc_eventbus::recv::SCEvent;
use std::any::Any;
use std::sync::Arc;

/// 数据包发送事件（可取消）：`set_cancelled(true)` 取消本次发送。
///
/// 负载仅携带数据包标识（目标/包 id/类型名）；数据包内容通过 `&dyn Any`
/// 分发参数以只读方式访问（`downcast_ref::<T>()` 可检查，不支持替换）。
///
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

/// 数据包发送钩子（异步，在 `send_packet` 发送前逐个 await）。
///
/// 两种能力：
/// - 取消：`event.set_cancelled(true)`；
/// - 检查：`packet.downcast_ref::<T>()` 只读数据包视图。
///
/// Packet send hook (async, awaited one by one before `send_packet` sends).
///
/// Two abilities:
/// - Cancel: `event.set_cancelled(true)`;
/// - Inspect: `packet.downcast_ref::<T>()` read-only packet view.
#[async_trait]
pub trait PacketSendHook: Send + Sync {
    async fn on_send(&self, event: &mut SCEvent<PacketSendEvent>, packet: &(dyn Any + Send + Sync));
}

/// 数据包钩子注册表。
///
/// Packet hook registry.
#[derive(Resource, Default)]
pub struct PacketSendHooks {
    hooks: RwLock<Vec<Arc<dyn PacketSendHook>>>,
}

impl PacketSendHooks {
    /// 注册一个钩子。
    ///
    /// Register a hook.
    pub fn register(&self, hook: Arc<dyn PacketSendHook>) {
        self.hooks.write().push(hook);
    }

    /// 清除所有钩子。
    ///
    /// Clear all hooks.
    pub fn clear(&self) {
        self.hooks.write().clear();
    }

    /// 是否存在任何钩子（为空时跳过分发）。
    ///
    /// Whether any hook exists (dispatch is skipped when empty).
    pub fn is_empty(&self) -> bool {
        self.hooks.read().is_empty()
    }

    pub(crate) fn snapshot(&self) -> Vec<Arc<dyn PacketSendHook>> {
        self.hooks.read().clone()
    }

    /// 分发：构造一个可取消事件，逐个 await 钩子（取消时短路），
    /// 然后返回本次发送是否被取消。
    ///
    /// - 调用前会快照钩子列表（Arc clone），以避免 register/clear 死锁；
    /// - `packet` 引用在 await 期间保持有效（`send_packet` 的调用方会持有
    ///   数据包直到发送完成）。
    ///
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
