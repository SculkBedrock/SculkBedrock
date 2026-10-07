//! Game-domain region extension (framework region types live in sc_ecs::region).
//!
//! This module keeps only the Minecraft-specific parts:
//! - [`RegionId`]: world plus grid-coordinate region identity;
//! - [`RegionGrid`]: chunk to region grid math;
//! - [`region_tick`]: sample region tick injected into `RegionDriver`.
//!
//! Shared framework types re-export here for game-domain use.

pub use sc_ecs::region::{
    EntityRegion, Region, RegionBus, RegionDriver, RegionEvent, RegionHub, RegionStatus,
};

use parking_lot::RwLock;
use sc_ecs::resource::Resource;
use sc_ecs::world::World;
use sc_entity::motion::{PhysicsBody, Transform};
use sc_utils::world::r#type::WorldType;
use sc_world::manager::MinecraftWorldManager;
use std::sync::Arc;
use std::time::Duration;

use crate::bus::{RegionPing, RegionPingBus};
use crate::world::GameWorldId;

/// Region identity (in-world grid coordinates).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct RegionId {
    pub world: GameWorldId,
    pub x: i32,
    pub z: i32,
}

impl RegionId {
    pub const fn new(world: GameWorldId, x: i32, z: i32) -> Self {
        Self { world, x, z }
    }
}

/// Region grid: `region_size_chunks` (NxN chunks per region) sets granularity.
///
/// Smaller regions parallelize better with more cross-region traffic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegionGrid {
    pub world: GameWorldId,
    /// Edge length per region in chunks (at least 1).
    pub region_size_chunks: u32,
}

impl RegionGrid {
    pub fn new(world: GameWorldId, region_size_chunks: u32) -> Self {
        Self {
            world,
            region_size_chunks: region_size_chunks.max(1),
        }
    }

    /// Owning region of chunk coordinates.
    #[inline]
    pub fn region_of(&self, chunk_x: i32, chunk_z: i32) -> RegionId {
        let n = self.region_size_chunks as i32;
        RegionId::new(self.world, chunk_x.div_euclid(n), chunk_z.div_euclid(n))
    }

    /// Origin chunk of a region.
    #[inline]
    pub fn region_origin(&self, region: RegionId) -> (i32, i32) {
        let n = self.region_size_chunks as i32;
        (region.x * n, region.z * n)
    }

    /// Regions within radius of a center region, center included.
    pub fn regions_in_radius(&self, center: RegionId, radius: u32) -> Vec<RegionId> {
        let radius = radius as i32;
        let mut out = Vec::with_capacity(((radius * 2 + 1) as usize).pow(2));
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                out.push(RegionId::new(self.world, center.x + dx, center.z + dz));
            }
        }
        out
    }
}

/// Region command: registered into [`RegionCommands`], run during the region tick.
pub trait RegionCommand: Send + Sync {
    fn run(&self, world: &World, region: RegionId);
}

/// Region command set (resource on the partitioned world).
#[derive(Resource, Default)]
pub struct RegionCommands {
    commands: RwLock<Vec<Arc<dyn RegionCommand>>>,
}

impl RegionCommands {
    pub fn register(&self, command: Arc<dyn RegionCommand>) {
        self.commands.write().push(command);
    }

    pub fn clear(&self) {
        self.commands.write().clear();
    }

    /// Run snapshotted commands without holding locks across calls.
    pub fn run_all(&self, world: &World, region: RegionId) {
        let commands = self.commands.read().clone();
        for command in commands {
            command.run(world, region);
        }
    }
}

/// Region physics command wrapping [`region_physics`].
pub struct RegionPhysicsCommand;

impl RegionCommand for RegionPhysicsCommand {
    fn run(&self, world: &World, region: RegionId) {
        region_physics(world, region);
    }
}

/// Single region tick (sample game-side implementation).
///
/// Counts ticks, processes inbound migration, exchanges neighbor pings,
/// and runs registered region commands such as physics.
pub fn region_tick(world: &World, region: RegionId) {
    // Demo switches (production mode disables pings and migration demos).
    let demo = world
        .get_resource::<crate::plugin::DemoSystems>()
        .map(|d| d.0)
        .unwrap_or(false);

    // Process entities migrated into this region.
    if let Some(mut hub) = world.get_resource_mut::<RegionHub<RegionId>>() {
        hub.recover_expired_migrations(world, Duration::from_secs(5));
        hub.process_inbound(world, &region);
    }

    // Receive this region's events.
    if let Some(mut bus) = world.get_resource_mut::<RegionPingBus>() {
        let events = bus.take_for(&region);
        if !events.is_empty() {
            log::debug!(
                "region ({},{}) received {} region events",
                region.x,
                region.z,
                events.len()
            );
        }
    }

    // Advance this region's tick.
    let (tick, should_migrate, should_ping) = {
        let Some(mut hub) = world.get_resource_mut::<RegionHub<RegionId>>() else {
            return;
        };
        let Some(region_state) = hub.get_mut(&region) else {
            return;
        };
        region_state.tick += 1;
        (
            region_state.tick,
            demo && region_state.tick % 200 == 0,
            demo && region_state.tick % 100 == 0,
        )
    };

    // Demo: ping neighbor regions every 100 ticks.
    if should_ping {
        if let Some(mut bus) = world.get_resource_mut::<RegionPingBus>() {
            let neighbor = RegionId::new(region.world, (region.x + 1) % 2, region.z);
            bus.send(region, neighbor, RegionPing { from_tick: tick });
        }
    }

    // Demo: migrate this region's first entity to the right neighbor.
    if should_migrate {
        let entity = world
            .entities_with_component::<EntityRegion<RegionId>>()
            .into_iter()
            .find(|entity| {
                world
                    .get_component::<EntityRegion<RegionId>>(entity)
                    .map(|er| er.0 == region)
                    .unwrap_or(false)
            });
        if let Some(entity) = entity {
            let neighbor = RegionId::new(region.world, (region.x + 1) % 2, region.z);
            let Some(mut hub) = world.get_resource_mut::<RegionHub<RegionId>>() else {
                return;
            };
            if !hub.begin_migration(world, entity, region, neighbor) {
                log::debug!("migration failed: entity missing or without EntityRegion");
            }
        }
    }

    // Region command set when registered, direct physics otherwise.
    if let Some(commands) = world.get_resource::<RegionCommands>() {
        commands.run_all(world, region);
    } else {
        region_physics(world, region);
    }

    // Periodic marker proving independent region progress.
    if tick % 100 == 0 {
        if let Some(hub) = world.get_resource::<RegionHub<RegionId>>() {
            let Some(region_state) = hub.get(&region) else {
                return;
            };
            let entity_count = world
                .entities_with_component::<EntityRegion<RegionId>>()
                .into_iter()
                .filter(|entity| {
                    world
                        .get_component::<EntityRegion<RegionId>>(entity)
                        .map(|er| er.0 == region)
                        .unwrap_or(false)
                })
                .count();
            log::debug!(
                "region ({},{}) tick={} entity count={}",
                region.x,
                region.z,
                region_state.tick,
                entity_count
            );
        }
    }
}

/// In-region game commands (physics): gravity and Y collision for this
/// region's entities, reading shared world data without IO.
fn region_physics(world: &World, region: RegionId) {
    let Some(manager) = world.get_resource::<MinecraftWorldManager>() else {
        return;
    };
    let Some(world_id) = manager
        .get_worlds_by_type(&WorldType::Overworld)
        .first()
        .map(|w| w.world_id.clone())
    else {
        return;
    };
    for entity in world.entities_with_component::<EntityRegion<RegionId>>() {
        let Some(er) = world.get_component::<EntityRegion<RegionId>>(&entity) else {
            continue;
        };
        if er.0 != region {
            continue;
        }
        let Some(transform) = world.get_component::<Transform>(&entity) else {
            continue;
        };
        let Some(physics) = world.get_component::<PhysicsBody>(&entity) else {
            continue;
        };
        let mut tf = transform.inner.write();
        if !physics.no_clip && !physics.flying {
            tf.velocity.y -= physics.gravity;
            if tf.velocity.y < -physics.terminal_velocity {
                tf.velocity.y = -physics.terminal_velocity;
            }
        }
        let target = physics
            .aabb
            .at(tf.position.x, tf.position.y + tf.velocity.y, tf.position.z);
        if !crate::movement::world_collides(&manager, &world_id, &target) {
            tf.position.y += tf.velocity.y;
            tf.on_ground = false;
        } else {
            if tf.velocity.y < 0.0 {
                tf.on_ground = true;
            }
            tf.velocity.y = 0.0;
        }
    }
}
