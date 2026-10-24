//! 权威摔落伤害应用（仅 owner tick）。
//!
//! Authoritative fall-damage application (owner-tick only).
//!
//! 生命值存放在 `EntityAttributes.minecraft:health.current_value`（默认 20）。
//! 本模块在 owner tick 下修改它并发布可靠事实；网络域拥有线路编码。
//! 失血不会因 outbox 受压而回滚：拒绝会经 `IntentPublisher` 升级为连接隔离
//! （与其他无重同步的生命周期事实同路径）。
//! Health lives in `EntityAttributes.minecraft:health.current_value` (default
//! 20). This module mutates it under the owner tick and publishes reliable
//! facts; the network domain owns wire encoding. Blood-loss is never rolled
//! back on outbox pressure: rejection escalates to connection isolation via
//! `IntentPublisher` (same path as other lifecycle facts without resync).

use sc_ecs::component::Component;
use sc_ecs::world::World;
use sc_entity::MinecraftEntityId;

use crate::damage::{calc_fall_damage, FallDamageInput, FallDamageOutcome, LandingMaterial};
use crate::net::{HurtEntity, IntentPublisher, SyncHealth};

/// 死亡标记：致命伤害后阻止重复的摔落结算。重生/复活会移除它
/// （当前只由本模块设置）。
///
/// Dead marker: stops repeat fall settlement after fatal damage.
/// Respawn / revive removes it (currently only set here).
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Dead(pub bool);

/// 一次权威摔落结算的结果。
///
/// Result of one authoritative fall settlement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FallSettlement {
    /// 本次结算造成的伤害。
    ///
    /// Damage applied by this settlement.
    pub damage: f32,
    /// 结算后的生命值。
    ///
    /// Health after the settlement.
    pub health: f32,
    /// 是否致命。
    ///
    /// Whether the settlement was fatal.
    pub fatal: bool,
}

/// 判定落地类型并运行纯公式。除调用方传入的 `sneaking` 标志外是纯函数
/// （此处不访问 World，便于测试）。
///
/// Classify landing + run the pure formula. Pure except for the `sneaking`
/// flag passed in by the caller (no World access here for testability).
///
/// - `fall_distance`: 摔落距离
/// - `landing`: 落地材料
/// - `sneaking`: 是否潜行
/// - `immune_gamemode`: 游戏模式免疫（创造/旁观/无敌）
/// - `can_fly`: 飞行能力免疫
/// - `gamerule_enabled`: 世界 `fallDamage` 游戏规则是否启用
/// - `is_player`: 是否为玩家
pub fn decide_fall_damage(
    fall_distance: f32,
    landing: LandingMaterial,
    sneaking: bool,
    immune_gamemode: bool,
    can_fly: bool,
    gamerule_enabled: bool,
    is_player: bool,
) -> FallDamageOutcome {
    calc_fall_damage(FallDamageInput {
        fall_distance,
        landing,
        sneaking,
        immune_gamemode,
        can_fly,
        gamerule_enabled,
        is_player,
        has_slow_falling: false,
        jump_boost: 0.0,
        ride_reduction: 0.0,
    })
}

/// 把结算后的摔落伤害应用到权威生命值并发布事实。
///
/// Apply settled fall damage to authoritative health and publish facts.
///
/// 无伤害时（门禁/偏移）返回 `None`，有伤害返回 `Some`。
/// Returns `None` when there is no damage (gates/offset), `Some` on damage.
///
/// - `world`: ECS 世界
/// - `entity`: 目标实体
/// - `runtime_id`: 实体运行时 id（intent 路由用）
/// - `damage`: 要应用的伤害
/// - `publisher`: intent 发布者
///
/// 步骤（全部同步，无 await，不持 Chunk/Network 锁）：
/// Steps (all synchronous, no await, no Chunk/Network locks held):
/// 1. 读 `EntityAttributes.minecraft:health`，扣减并钳制到 `[min,max]`；
///    read `EntityAttributes.minecraft:health`, subtract, clamp to
///    `[min,max]`;
/// 2. `health <= 0` 时设置 `Dead`；
///    set `Dead` on `health <= 0`;
/// 3. 经 `IntentPublisher` 发布 `SyncHealth` + `HurtEntity{fatal}`
///    （backpressure 升级为连接故障；生命值不回滚）。
///    publish `SyncHealth` + `HurtEntity{fatal}` via `IntentPublisher`
///    (backpressure escalates to connection fault; health is not rolled
///    back).
pub fn apply_fall_damage(
    world: &World,
    entity: &sc_ecs::entity::EntityId,
    runtime_id: u64,
    damage: f32,
    publisher: &mut IntentPublisher<'_>,
) -> Option<FallSettlement> {
    if !(damage.is_finite() && damage > 0.0) {
        return None;
    }
    if world.get_component::<Dead>(entity).is_some_and(|d| d.0) {
        return None;
    }
    let Some(attributes) =
        world.get_component::<sc_packloader::definitions::attribute::EntityAttributes>(entity)
    else {
        return None;
    };
    let health_key = sc_packloader::definitions::attribute::EntityAttributes::MAX_HEALTH;
    let Some(mut attr) = attributes.get_mut(health_key) else {
        return None;
    };
    let next = (attr.current_value - damage).clamp(attr.min_value, attr.max_value);
    attr.current_value = next;
    drop(attr);
    let fatal = next <= 0.0;
    if fatal {
        world.remove_component::<Dead>(entity);
        world.add_component(entity, Dead(true));
    }
    publisher.publish(SyncHealth {
        entity_id: runtime_id,
    });
    publisher.publish(HurtEntity {
        entity_id: runtime_id,
        fatal,
    });
    if fatal {
        // Gamerule opt-in skips the death screen through the same path.
        crate::respawn::maybe_immediate_respawn(world, entity, runtime_id, publisher);
    }
    // Keep the runtime-id index usable for the network mapping layer.
    let _ = world.get_component::<MinecraftEntityId>(entity);
    Some(FallSettlement {
        damage,
        health: next,
        fatal,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_ecs::world::World;
    use sc_entity::MinecraftEntityId;
    use sc_packloader::definitions::attribute::{EntityAttribute, EntityAttributes};

    fn world_with_health(health: f32) -> (World, sc_ecs::entity::EntityId) {
        let world = World::new();
        let entity = world.spawn(sc_utils::components::DisplayName("fall-test".to_string()));
        let mut map = std::collections::HashMap::new();
        map.insert(
            EntityAttributes::MAX_HEALTH.to_string(),
            EntityAttribute {
                min_value: 0.0,
                max_value: 20.0,
                default_min_value: 0.0,
                default_max_value: 20.0,
                default_value: 20.0,
                current_value: health,
                should_send: true,
            },
        );
        world.add_component(&entity, EntityAttributes::from_map(map));
        world.add_component(&entity, MinecraftEntityId(42));
        (world, entity)
    }

    #[test]
    fn non_positive_damage_applies_nothing() {
        let (world, entity) = world_with_health(20.0);
        let mut outbox = crate::net::NetworkOutbox::default();
        let mut publisher = IntentPublisher::new(&mut outbox);
        assert!(apply_fall_damage(&world, &entity, 42, 0.0, &mut publisher).is_none());
        assert!(outbox.is_empty());
    }

    #[test]
    fn damage_reduces_health_and_publishes_two_facts() {
        let (world, entity) = world_with_health(20.0);
        let mut outbox = crate::net::NetworkOutbox::default();
        let mut publisher = IntentPublisher::new(&mut outbox);
        let settlement =
            apply_fall_damage(&world, &entity, 42, 5.0, &mut publisher).expect("damage applies");
        assert_eq!(settlement.health, 15.0);
        assert!(!settlement.fatal);
        assert_eq!(outbox.pending(), 2);
    }

    #[test]
    fn lethal_damage_marks_dead_and_reports_fatal() {
        let (world, entity) = world_with_health(5.0);
        let mut outbox = crate::net::NetworkOutbox::default();
        let mut publisher = IntentPublisher::new(&mut outbox);
        let settlement =
            apply_fall_damage(&world, &entity, 42, 10.0, &mut publisher).expect("lethal applies");
        assert!(settlement.fatal);
        assert_eq!(settlement.health, 0.0);
        // Second settlement is suppressed while dead.
        let mut outbox2 = crate::net::NetworkOutbox::default();
        let mut publisher2 = IntentPublisher::new(&mut outbox2);
        assert!(apply_fall_damage(&world, &entity, 42, 5.0, &mut publisher2).is_none());
    }
}
