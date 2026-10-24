//! 玩家氧气消耗、溺水伤害与气泡恢复机制。
//!
//! Oxygen consumption, drowning damage, and air recovery for players.
//!
//! 规则实现（参考 Bedrock 官方行为与 Minecraft Wiki）：
//! - 氧气上限：从 `EntityAttributes::AIR`（`"sc:player.air"`）动态读取，原版默认为 300 ticks（10 颗气泡，每颗 30 ticks）；
//! - 头部浸水：当玩家眼部/头部浸没在静止水（`minecraft:water`）或流动水（`minecraft:flowing_water`）中且非气泡柱时，
//!   处于缺氧状态（`is_breathing = false`），每 tick 消耗 1 tick 氧气；
//! - 气泡柱恢复：当玩家处于气泡柱（`minecraft:bubble_column`）中，即使头部浸水也能正常呼吸并恢复氧气；
//! - 溺水伤害：氧气降为 0 时开始溺水计时，每 20 ticks（1 秒）受到 2.0 点溺水伤害（`EntityAttributes::MAX_HEALTH`），
//!   发布 [`SyncHealth`] 与 [`HurtEntity`]；生命值耗尽时标记 [`Dead`]；
//! - 氧气恢复：当头部离开水面或进入气泡柱时，处于呼吸状态（`is_breathing = true`），
//!   以每 tick +5.0 ticks 的速度快速恢复，直至达到 `air.max_value`；
//! - 网络同步：通过 [`SyncAirSupply`] 向客户端同步 `air_supply`、`max_air` 与 `is_breathing`，
//!   并在呼吸状态改变、耗尽、充满或每累计改变 20 ticks 时限流发布。

use std::sync::Arc;
use parking_lot::RwLock;

use sc_ecs::component::Component;
use sc_ecs::entity::EntityId;
use sc_ecs::params::resource::ResMut;
use sc_ecs::world::World;
use sc_entity::motion::{PhysicsBody, Position, Transform};
use sc_packloader::definitions::attribute::EntityAttributes;
use sc_world::block_dictionary::BlockStateDictionary;
use sc_world::chunk::ChunkPosition;
use sc_world::manager::{MinecraftWorldId, MinecraftWorldManager};
use sc_world::storage::ChunkKey;

use crate::damage::apply::Dead;
use crate::net::{HurtEntity, IntentPublisher, NetworkOutbox, SyncAirSupply, SyncHealth};

/// 氧气默认上限（300 ticks = 15 秒 = 10 颗气泡）。
pub const DEFAULT_MAX_AIR: f32 = 300.0;
/// 氧气在空气或气泡柱中的每 tick 恢复速率。
pub const AIR_RECOVERY_PER_TICK: f32 = 5.0;
/// 氧气耗尽后的溺水伤害触发间隔（20 ticks = 1 秒）。
pub const DROWNING_INTERVAL_TICKS: u8 = 20;
/// 每次溺水伤害点数（2.0 = 1 颗心）。
pub const DROWNING_DAMAGE: f32 = 2.0;
/// 网络同步限流阈值（氧气变化累计达到此 ticks 时触发一次同步）。
pub const AIR_SYNC_THRESHOLD_TICKS: i16 = 20;

/// 仅服务端使用的玩家氧气与溺水阶段计时器。
///
/// Server-only player oxygen and drowning phase clock.
#[derive(Component, Clone, Debug, Default)]
pub struct OxygenClock {
    pub inner: Arc<RwLock<OxygenClockData>>,
}

/// 玩家氧气计时状态快照。
///
/// Oxygen clock state snapshot.
#[derive(Clone, Copy, Debug)]
pub struct OxygenClockData {
    /// 缺氧时累计的溺水伤害 tick 计数器（每 20 ticks 造成一次溺水伤害）。
    pub drowning_ticks: u8,
    /// 呼吸状态（在水中无气泡柱为 false，陆地/水面/气泡柱中为 true）。
    pub is_breathing: bool,
    /// 上次向客户端网络同步时的氧气值（用于限流）。
    pub last_synced_air: i16,
}

impl Default for OxygenClockData {
    fn default() -> Self {
        Self {
            drowning_ticks: 0,
            is_breathing: true,
            last_synced_air: DEFAULT_MAX_AIR as i16,
        }
    }
}

impl OxygenClock {
    pub fn new() -> Self {
        Self::default()
    }
}

/// 检查实体头部/眼部是否完全浸没在会导致窒息的水体中（普通水/流动水，排除气泡柱）。
pub fn is_head_submerged_in_drowning_water(
    world_manager: &MinecraftWorldManager,
    world_id: &MinecraftWorldId,
    pos: &Position,
    height: f32,
) -> bool {
    let Some(minecraft_world) = world_manager.get_world(world_id) else {
        return false;
    };
    let bx = pos.x.floor() as i32;
    let bz = pos.z.floor() as i32;
    // Eye/head level: if player is swimming/crawling (height < 1.0), eye is ~0.4;
    // otherwise standing (height ~ 1.8), eye is ~1.62 (or 0.85 * height).
    let eye_offset = if height < 1.0 { 0.4 * height } else { 0.85 * height };
    let head_y = (pos.y + eye_offset).floor() as i32;

    let dimension = minecraft_world.world_data.get_dimension();
    let key = ChunkKey::new(dimension, ChunkPosition::from_world(bx, bz));
    let Some(column) = minecraft_world.chunk_provider.cached_chunk(key) else {
        return false;
    };
    let chunk = column.read();
    let lx = bx.rem_euclid(16) as u8;
    let lz = bz.rem_euclid(16) as u8;

    let dict = BlockStateDictionary::global();
    let is_bubble_column = |id: Option<sc_world::chunk::BlockRuntimeId>| {
        id.is_some_and(|id| {
            dict.get(id.0)
                .is_some_and(|e| e.name == "minecraft:bubble_column")
        })
    };
    let is_drowning_water = |id: Option<sc_world::chunk::BlockRuntimeId>| {
        id.is_some_and(|id| {
            dict.get(id.0).is_some_and(|e| {
                e.name == "minecraft:water" || e.name == "minecraft:flowing_water"
            })
        })
    };

    let b0 = chunk.block_at_layer(0, lx, head_y, lz);
    let b1 = chunk.block_at_layer(1, lx, head_y, lz);

    // Bubble columns provide breathable air!
    if is_bubble_column(b0) || is_bubble_column(b1) {
        return false;
    }

    is_drowning_water(b0) || is_drowning_water(b1)
}

/// 执行单实体单 tick 氧气与溺水逻辑。
///
/// Returns true if synced state changed.
pub fn tick_oxygen(
    world: &World,
    entity: &EntityId,
    runtime_id: u64,
    publisher: &mut IntentPublisher<'_>,
) -> bool {
    if world.get_component::<Dead>(entity).is_some_and(|d| d.0) {
        return false;
    }
    use sc_utils::game::gamemode::Gamemode;
    let Some(client) = world.get_component::<sc_utils::game::client::MinecraftClient>(entity)
    else {
        return false;
    };
    let gamemode = client.data.read().gamemode;
    let Some(attributes) = world.get_component::<EntityAttributes>(entity) else {
        return false;
    };
    let Some(mut air) = attributes.get_mut(EntityAttributes::AIR) else {
        return false;
    };

    // Creative / Spectator do not consume air or drown.
    if !matches!(gamemode, Gamemode::Survival | Gamemode::Adventure) {
        if (air.current_value - air.max_value).abs() > f32::EPSILON {
            air.current_value = air.max_value;
            publisher.publish(SyncAirSupply {
                entity_id: runtime_id,
                air_supply: air.max_value as i16,
                max_air: air.max_value as i16,
                is_breathing: true,
            });
            return true;
        }
        return false;
    }

    let (pos, height) = match (
        world.get_component::<Transform>(entity),
        world.get_component::<PhysicsBody>(entity),
    ) {
        (Some(tf), Some(body)) => {
            let p = tf.inner.read().position;
            let h = body.aabb.max_y;
            (p, h)
        }
        _ => return false,
    };
    let Some(world_id) = world.get_component::<MinecraftWorldId>(entity) else {
        return false;
    };
    let Some(world_manager) = world.get_resource::<MinecraftWorldManager>() else {
        return false;
    };

    let clock = match world.get_component::<OxygenClock>(entity) {
        Some(clock) => clock,
        None => {
            world.add_component(entity, OxygenClock::new());
            let Some(clock) = world.get_component::<OxygenClock>(entity) else {
                return false;
            };
            clock
        }
    };
    let mut clock_data = clock.inner.write();

    let head_underwater =
        is_head_submerged_in_drowning_water(&world_manager, &world_id, &pos, height);
    let was_breathing = clock_data.is_breathing;
    let is_breathing = !head_underwater;
    clock_data.is_breathing = is_breathing;

    let mut state_changed = false;

    if head_underwater {
        // Consuming air underwater (-1.0 tick/tick).
        air.current_value = (air.current_value - 1.0).max(0.0);
        if air.current_value <= 0.0 {
            clock_data.drowning_ticks = clock_data.drowning_ticks.saturating_add(1);
            if clock_data.drowning_ticks >= DROWNING_INTERVAL_TICKS {
                clock_data.drowning_ticks = 0;
                // Deal drowning damage (2.0 dmg = 1 heart)
                let mut fatal = false;
                if let Some(mut health) = attributes.get_mut(EntityAttributes::MAX_HEALTH) {
                    health.current_value = (health.current_value - DROWNING_DAMAGE).max(0.0);
                    if health.current_value <= 0.0 {
                        fatal = true;
                    }
                }
                if fatal {
                    world.remove_component::<Dead>(entity);
                    world.add_component(entity, Dead(true));
                    publisher.publish(SyncHealth {
                        entity_id: runtime_id,
                    });
                    publisher.publish(HurtEntity {
                        entity_id: runtime_id,
                        fatal: true,
                    });
                    crate::respawn::maybe_immediate_respawn(world, entity, runtime_id, publisher);
                } else {
                    publisher.publish(SyncHealth {
                        entity_id: runtime_id,
                    });
                    publisher.publish(HurtEntity {
                        entity_id: runtime_id,
                        fatal: false,
                    });
                }
                state_changed = true;
            }
        }
    } else {
        // In air or bubble column: reset drowning damage timer and recover air (+5.0 ticks/tick).
        clock_data.drowning_ticks = 0;
        if air.current_value < air.max_value {
            air.current_value = (air.current_value + AIR_RECOVERY_PER_TICK).min(air.max_value);
        }
    }

    let air_i16 = air.current_value.round() as i16;
    let max_air_i16 = air.max_value.round() as i16;

    // Network sync gating:
    // 1. Breathing state changed (entering / exiting water)
    // 2. Air reached zero
    // 3. Air reached maximum
    // 4. Accumulated delta >= AIR_SYNC_THRESHOLD_TICKS
    let breathing_changed = was_breathing != is_breathing;
    let reached_zero = air_i16 == 0 && clock_data.last_synced_air != 0;
    let reached_max = air_i16 == max_air_i16 && clock_data.last_synced_air != max_air_i16;
    let delta_exceeded =
        (clock_data.last_synced_air - air_i16).abs() >= AIR_SYNC_THRESHOLD_TICKS;

    if breathing_changed || reached_zero || reached_max || delta_exceeded {
        clock_data.last_synced_air = air_i16;
        publisher.publish(SyncAirSupply {
            entity_id: runtime_id,
            air_supply: air_i16,
            max_air: max_air_i16,
            is_breathing,
        });
        state_changed = true;
    }

    state_changed
}

/// 每 tick 氧气与溺水系统驱动（在 `sc_ecs::schedule::Update` 中运行）。
pub fn oxygen_tick(world: World, mut outbox: ResMut<NetworkOutbox>) {
    let mut publisher = IntentPublisher::new(&mut outbox);
    for entity in world.entities_with_component::<sc_entity::MinecraftEntityId>() {
        let Some(actor) = world.get_component::<sc_entity::MinecraftEntityId>(&entity) else {
            continue;
        };
        // Oxygen is a player system; entities without a player inventory
        // (mobs, items, markers) never tick here.
        if world
            .get_component::<sc_item::PlayerInventory>(&entity)
            .is_none()
        {
            continue;
        }
        tick_oxygen(&world, &entity, actor.0, &mut publisher);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_entity::motion::Rotation;
    use sc_entity::MinecraftEntityId;
    use sc_item::PlayerInventory;
    use sc_packloader::definitions::attribute::{EntityAttribute, EntityAttributes};
    use sc_utils::game::client::MinecraftClient;
    use sc_utils::game::difficulty::Difficulty;
    use sc_utils::world::client_data::MinecraftClientData;

    fn attribute(value: f32, max: f32) -> EntityAttribute {
        EntityAttribute {
            min_value: 0.0,
            max_value: max,
            default_min_value: 0.0,
            default_max_value: max,
            default_value: value,
            current_value: value,
            should_send: false,
        }
    }

    fn survival_player_world() -> (World, EntityId) {
        let world = World::new();
        let entity = world.spawn(sc_utils::components::DisplayName("oxygen-test".to_string()));
        let mut map = std::collections::HashMap::new();
        map.insert(
            EntityAttributes::MAX_HEALTH.to_string(),
            attribute(20.0, 20.0),
        );
        map.insert(
            EntityAttributes::AIR.to_string(),
            attribute(300.0, 300.0),
        );
        world.add_component(&entity, EntityAttributes::from_map(map));
        world.add_component(&entity, PlayerInventory::new(36));
        world.add_component(&entity, MinecraftEntityId(12));
        world.add_component(
            &entity,
            Transform::new(Position::new(0.0, 64.0, 0.0), Rotation::default()),
        );
        world.add_component(&entity, PhysicsBody::player());
        let client = MinecraftClient::new(entity, world.clone(), MinecraftClientData::default());
        world.add_component(&entity, client);

        let mut manager = MinecraftWorldManager::new();
        let mut data = sc_utils::world::data::MinecraftWorldData::default();
        data.world_type = Some(sc_utils::world::r#type::WorldType::Overworld);
        data.difficulty = Difficulty::Normal;
        let provider = sc_world::storage::WorldChunkProvider::new(std::sync::Arc::new(
            sc_world::storage::EmptyWorldStorage,
        ));
        let world_id = manager.push_world(
            sc_utils::world::r#type::WorldType::Overworld,
            sc_world::world::MinecraftWorld::new(
                sc_world::manager::MinecraftWorldId::random(),
                "test".into(),
                std::path::PathBuf::new(),
                data,
                provider,
            ),
        );
        world.add_component(&entity, world_id);
        world.insert_resource(manager);
        (world, entity)
    }

    #[test]
    fn air_is_full_in_dry_air() {
        let (world, entity) = survival_player_world();
        let mut outbox = NetworkOutbox::default();
        let mut publisher = IntentPublisher::new(&mut outbox);

        // Dry air (empty world, air block everywhere).
        // 50 ticks of ticking should keep air at 300.0.
        for _ in 0..50 {
            tick_oxygen(&world, &entity, 12, &mut publisher);
        }

        let attributes = world
            .get_component::<EntityAttributes>(&entity)
            .expect("attributes");
        let air = attributes.get(EntityAttributes::AIR).expect("air");
        assert_eq!(air.current_value, 300.0);
    }

    #[test]
    fn air_recovers_at_five_per_tick_when_breathing() {
        let (world, entity) = survival_player_world();
        let attributes = world
            .get_component::<EntityAttributes>(&entity)
            .expect("attributes");
        attributes.get_mut(EntityAttributes::AIR).expect("air").current_value = 100.0;

        let mut outbox = NetworkOutbox::default();
        let mut publisher = IntentPublisher::new(&mut outbox);

        // 10 ticks in air: 100 + 10 * 5 = 150.
        for _ in 0..10 {
            tick_oxygen(&world, &entity, 12, &mut publisher);
        }

        let air = attributes.get(EntityAttributes::AIR).expect("air");
        assert_eq!(air.current_value, 150.0);
    }

    #[test]
    fn underwater_consumes_air_and_deals_drowning_damage() {
        let (world, entity) = survival_player_world();
        let water_id = sc_world::block_dictionary::resolve_or_register_default("minecraft:water");
        let world_id = world.get_component::<MinecraftWorldId>(&entity).unwrap();
        let manager = world.get_resource::<MinecraftWorldManager>().unwrap();
        let minecraft_world = manager.get_world(&world_id).unwrap();
        let key = ChunkKey::new(0, ChunkPosition::new(0, 0));
        let column = minecraft_world.chunk_provider.insert_empty_chunk(key, -64, 319).unwrap();
        // Place water at player eye level (y = 65)
        column.write().set_block_at(0, 0, 65, 0, sc_world::chunk::BlockRuntimeId(water_id));

        let mut outbox = NetworkOutbox::default();
        let mut publisher = IntentPublisher::new(&mut outbox);

        // 1 tick underwater: air decreases from 300 to 299
        tick_oxygen(&world, &entity, 12, &mut publisher);
        let attributes = world.get_component::<EntityAttributes>(&entity).unwrap();
        assert_eq!(attributes.get(EntityAttributes::AIR).unwrap().current_value, 299.0);

        // Drain air to 0
        attributes.get_mut(EntityAttributes::AIR).unwrap().current_value = 0.0;
        // Ticks 1-19 at 0 air do not deal damage
        for _ in 0..19 {
            tick_oxygen(&world, &entity, 12, &mut publisher);
        }
        assert_eq!(attributes.get(EntityAttributes::MAX_HEALTH).unwrap().current_value, 20.0);

        // 20th tick at 0 air deals 2.0 drowning damage
        tick_oxygen(&world, &entity, 12, &mut publisher);
        assert_eq!(attributes.get(EntityAttributes::MAX_HEALTH).unwrap().current_value, 18.0);
    }

    #[test]
    fn bubble_column_restores_air_underwater() {
        let (world, entity) = survival_player_world();
        let bubble_id = sc_world::block_dictionary::resolve_or_register_default("minecraft:bubble_column");
        let world_id = world.get_component::<MinecraftWorldId>(&entity).unwrap();
        let manager = world.get_resource::<MinecraftWorldManager>().unwrap();
        let minecraft_world = manager.get_world(&world_id).unwrap();
        let key = ChunkKey::new(0, ChunkPosition::new(0, 0));
        let column = minecraft_world.chunk_provider.insert_empty_chunk(key, -64, 319).unwrap();
        // Place bubble column at player eye level (y = 65)
        column.write().set_block_at(0, 0, 65, 0, sc_world::chunk::BlockRuntimeId(bubble_id));

        let attributes = world.get_component::<EntityAttributes>(&entity).unwrap();
        attributes.get_mut(EntityAttributes::AIR).unwrap().current_value = 100.0;

        let mut outbox = NetworkOutbox::default();
        let mut publisher = IntentPublisher::new(&mut outbox);

        // Submerged in bubble column: should breathe and restore air (+5/tick)
        tick_oxygen(&world, &entity, 12, &mut publisher);
        assert_eq!(attributes.get(EntityAttributes::AIR).unwrap().current_value, 105.0);
    }

    #[test]
    fn drowning_fatal_marks_dead() {
        let (world, entity) = survival_player_world();
        let water_id = sc_world::block_dictionary::resolve_or_register_default("minecraft:water");
        let world_id = world.get_component::<MinecraftWorldId>(&entity).unwrap();
        let manager = world.get_resource::<MinecraftWorldManager>().unwrap();
        let minecraft_world = manager.get_world(&world_id).unwrap();
        let key = ChunkKey::new(0, ChunkPosition::new(0, 0));
        let column = minecraft_world.chunk_provider.insert_empty_chunk(key, -64, 319).unwrap();
        column.write().set_block_at(0, 0, 65, 0, sc_world::chunk::BlockRuntimeId(water_id));

        let attributes = world.get_component::<EntityAttributes>(&entity).unwrap();
        attributes.get_mut(EntityAttributes::AIR).unwrap().current_value = 0.0;
        attributes.get_mut(EntityAttributes::MAX_HEALTH).unwrap().current_value = 1.0;

        let mut outbox = NetworkOutbox::default();
        let mut publisher = IntentPublisher::new(&mut outbox);

        for _ in 0..20 {
            tick_oxygen(&world, &entity, 12, &mut publisher);
        }
        assert!(world.get_component::<Dead>(&entity).is_some_and(|d| d.0));
    }

    #[test]
    fn creative_mode_immune_to_drowning() {
        let (world, entity) = survival_player_world();
        let client = world.get_component::<MinecraftClient>(&entity).unwrap();
        client.data.write().gamemode = sc_utils::game::gamemode::Gamemode::Creative;

        let water_id = sc_world::block_dictionary::resolve_or_register_default("minecraft:water");
        let world_id = world.get_component::<MinecraftWorldId>(&entity).unwrap();
        let manager = world.get_resource::<MinecraftWorldManager>().unwrap();
        let minecraft_world = manager.get_world(&world_id).unwrap();
        let key = ChunkKey::new(0, ChunkPosition::new(0, 0));
        let column = minecraft_world.chunk_provider.insert_empty_chunk(key, -64, 319).unwrap();
        column.write().set_block_at(0, 0, 65, 0, sc_world::chunk::BlockRuntimeId(water_id));

        let attributes = world.get_component::<EntityAttributes>(&entity).unwrap();
        attributes.get_mut(EntityAttributes::AIR).unwrap().current_value = 50.0;

        let mut outbox = NetworkOutbox::default();
        let mut publisher = IntentPublisher::new(&mut outbox);

        tick_oxygen(&world, &entity, 12, &mut publisher);
        // Restores to 300.0 immediately and does not drown
        assert_eq!(attributes.get(EntityAttributes::AIR).unwrap().current_value, 300.0);
        assert_eq!(attributes.get(EntityAttributes::MAX_HEALTH).unwrap().current_value, 20.0);
    }
}
