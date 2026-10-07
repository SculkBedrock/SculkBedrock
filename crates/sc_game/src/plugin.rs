//! `SCGamePlugin`: game-domain schedule plugin.
//!
//! Registers `WorldHub`/bus/outbox resources plus the default world instance,
//! and game commands (Update: multi-world hub_tick; PostUpdate: cross-world
//! dispatch, outbound intent skeleton).

use sc_ecs::app::plugin::Plugin;
use sc_ecs::app::App;
use sc_ecs::params::event::EventReader;
use sc_ecs::params::local::Local;
use sc_ecs::params::resource::{Res, ResMut};
use sc_ecs::world::World;
use sc_log::t_log;
use sc_utils::event::SCExit;
use sc_utils::game::client::MinecraftClient;
use sc_utils::game::structs::position::MinecraftPosition;
use sc_utils::world::r#type::WorldType;
use sc_world::chunk_view::ChunkView;
use sc_world::manager::{MinecraftWorldId, MinecraftWorldManager};
use std::sync::Arc;
use std::time::Duration;

use crate::block::{block_changed_to_outbox, DropReceipts};
use crate::bus::{
    AnnouncementBus, CrossWorldPing, PingBus, PingMailbox, RegionPingBus, TeleportBus,
    TeleportRequest, WorldAnnouncement,
};
use crate::interaction::BlockPlacementReservations;
use crate::item_drop::{item_drop_tick, item_pickup_tick, ItemDropStore};
use crate::movement::{movement_broadcast, movement_input_drain, movement_physics, PlayerMovement};
use crate::net::{
    IntentReliability, IntentSendHooks, NetworkIntent, NetworkOutbox, PlayerMoveMode,
};
use crate::net_backpressure::{
    flush_pending_inventory_resync, IntentPublisher, PendingInventoryResync,
};
use crate::net_faults::PendingConnectionFaults;
use crate::region::{
    region_tick, EntityRegion, RegionCommands, RegionDriver, RegionGrid, RegionHub, RegionId,
    RegionPhysicsCommand,
};
use crate::world::{
    GameWorldEntry, GameWorldId, GameWorldMap, PlayerGameWorld, TickCounter, WorldHub,
};
use sc_entity::motion::{PhysicsBody, Position, Rotation, Transform};
use sc_entity::MinecraftEntityId;
use sc_item::ItemRegistry;

/// Default game tick interval (20 TPS). Region drivers take any interval.
const GAME_TICK_INTERVAL: Duration = Duration::from_millis(50);
const TELEPORT_COMPLETE_TIMEOUT_TICKS: u32 = 100;

pub struct SCGamePlugin;

impl Plugin for SCGamePlugin {
    fn build(&self, app: &App) {
        app.insert_resource(WorldHub::default())
            .insert_resource(GameWorldMap::default())
            .insert_resource(PingBus::default())
            .insert_resource(TeleportBus::default())
            .insert_resource(AnnouncementBus::default())
            .insert_resource(NetworkOutbox::default())
            // Bounded fallback when outbound budget rejects reliable facts.
            .insert_resource(PendingInventoryResync::default())
            // Terminal states without resync paths register here for isolation.
            .insert_resource(PendingConnectionFaults::default())
            .insert_resource(BlockPlacementReservations::default())
            .insert_resource(ItemDropStore::default())
            // Break drop idempotency receipts.
            .insert_resource(DropReceipts::default())
            .insert_resource(IntentSendHooks::default())
            .insert_resource(RegionDriver::<RegionId>::default())
            // Item registry (item to block mapping; filled after version packs load).
            .insert_resource(ItemRegistry::default())
            // Recipe snapshot (compiled from behavior packs; empty when missing).
            .insert_resource(crate::crafting::SharedRecipeRegistry::default())
            .insert_resource(crate::crafting::SharedItemTags::default())
            // Demo systems default off, overridden from server properties.
            .insert_resource(DemoSystems(false))
            .add_systems(
                sc_utils::schedule::SCLoad,
                (
                    init_demo_flag,
                    spawn_default_world,
                    spawn_demo_regions,
                    spawn_dimensions,
                ),
            )
            // Game-side systems.
            .add_systems(sc_utils::schedule::SCMovementInput, movement_input_drain)
            // Rejected inventory facts requeue here, ahead of this tick's producers.
            .add_systems(
                sc_utils::schedule::SCMovementInput,
                flush_pending_inventory_resync,
            )
            .add_systems(sc_ecs::schedule::Update, movement_physics)
            .add_systems(sc_ecs::schedule::Update, item_drop_tick)
            // Pick up settled drops by player AABB.
            .add_systems(sc_ecs::schedule::Update, item_pickup_tick)
            .add_systems(sc_ecs::schedule::Update, demo_cross_world_transfer)
            .add_systems(sc_ecs::schedule::Update, update_player_region)
            // Authoritative break timing from block data; the server lands
            // breaks itself when time expires.
            .add_systems(
                sc_ecs::schedule::Update,
                crate::interaction::advance_block_break,
            )
            .add_systems(sc_utils::schedule::SCMovementBroadcast, movement_broadcast)
            .add_systems(sc_ecs::schedule::PostUpdate, block_changed_to_outbox)
            // Player place/break boundary events.
            .add_event::<crate::interaction::BreakBlockRequest>()
            .add_event::<crate::interaction::PlaceBlockRequest>()
            .add_event::<crate::interaction::HeldSlotChanged>()
            .add_event::<crate::interaction::StartBreakRequest>()
            .add_event::<crate::interaction::AbortBreakRequest>()
            .add_event::<crate::interaction::OpenInventoryRequest>()
            .add_event::<crate::interaction::CloseInventoryRequest>()
            .add_event::<crate::crafting::CraftRequestEvent>()
            .add_systems(
                sc_utils::schedule::SCEventUpdate,
                (
                    crate::interaction::drain_mining_actions,
                    crate::interaction::handle_start_break,
                    crate::interaction::handle_abort_break,
                    crate::interaction::handle_break_block,
                    crate::interaction::handle_place_block,
                    crate::interaction::handle_held_slot_changed,
                    crate::interaction::resolve_block_placement_reservations,
                    crate::interaction::handle_open_inventory,
                    crate::interaction::handle_close_inventory,
                    crate::crafting::handle_craft_requests,
                ),
            )
            // Shutdown: stop region threads on SCExit (e.g. stop command).
            .add_systems(sc_ecs::schedule::Last, on_exit_shutdown_regions)
            .add_systems(sc_ecs::schedule::Update, hub_tick)
            .add_systems(
                sc_ecs::schedule::PostUpdate,
                (
                    dispatch_ping_events,
                    process_teleports,
                    complete_teleports,
                    process_announcements,
                ),
            );
    }
}

/// Demo systems switch (resource) from server properties (default false).
///
/// Demos emit region events and entity migrations even with no players, so
/// production runs keep them off; the cross-world teleport demo would also
/// disturb real players.
#[derive(sc_ecs::resource::Resource, Clone, Copy, Debug, Default)]
pub struct DemoSystems(pub bool);

/// Load the demo switch from server properties (default off).
fn init_demo_flag(world: World, mut demo: ResMut<DemoSystems>) {
    if let Some(properties) =
        world.get_resource::<sc_utils::game::structs::server_properties::ServerProperties>()
    {
        demo.0 = properties.demo_systems;
    }
    if demo.0 {
        log::info!("{}", t_log!("console.game.demo_on"));
    } else {
        log::info!("{}", t_log!("console.game.demo_off"));
    }
}

/// Stop all region threads on SCExit (e.g. stop command).
fn on_exit_shutdown_regions(
    mut driver: ResMut<RegionDriver<RegionId>>,
    mut reader: EventReader<SCExit>,
) {
    if reader.read().next().is_some() {
        driver.shutdown();
        log::info!("{}", t_log!("console.game.regions_stopped"));
        log::info!("{}", t_log!("console.game.exit_done"));
            // Flush order: the last log line enters the sink before the log thread closes.
        sc_log::file::shutdown();
        if sc_utils::host_mode::is_hosted() {
            // Hosted mode (GUI/mobile embedding): only set the stopped flag;
            // the host runner polls it and returns normally.
            sc_utils::host_mode::notify_server_stopped();
        } else {
            // Graceful exit: stop command, Ctrl+C, and terminal close converge here.
            std::process::exit(0);
        }
    }
}

/// Mount the main world instance at startup (GameWorldId(0)).
fn spawn_default_world(mut hub: ResMut<WorldHub>) {
    if !hub.contains(&GameWorldId(0)) {
        hub.spawn_world(GameWorldId(0));
        log::info!("{}", t_log!("console.game.world_mounted"));
    }
}

/// Region demo: split the main world into 2x2 regions with demo entities.
///
/// Tick interval (50ms = 20 TPS) and the `region_tick` function come from
/// this plugin; `RegionDriver` itself takes any game parameters.
fn spawn_demo_regions(
    world: World,
    mut driver: ResMut<RegionDriver<RegionId>>,
    demo: Res<DemoSystems>,
) {
    // Take the main world's ECS World handle (shared with region threads).
    let child = {
        let Some(hub) = world.get_resource::<WorldHub>() else {
            return;
        };
        let Some(game) = hub.get(&GameWorldId(0)) else {
            return;
        };
        game.world.clone()
    };

    // Region registry and event bus live on the main world.
    child.insert_resource(RegionHub::<RegionId>::default());
    child.insert_resource(crate::bus::RegionPingBus::default());
    // Shared world manager: region threads read chunk data through it.
    if let Some(manager) = world.get_resource::<sc_world::manager::MinecraftWorldManager>() {
        child.insert_resource((*manager).clone());
    }
    // Sync the demo switch into child worlds.
    child.insert_resource(*demo);

    // Production mode (demo_systems=false) starts no demo region threads
    // or entities; player region ownership and dimensions are unaffected.
    if !demo.0 {
        log::info!("{}", t_log!("console.game.demo_skipped"));
        return;
    }

    let mut regions = Vec::new();
    for x in 0..2 {
        for z in 0..2 {
            let region = RegionId::new(GameWorldId(0), x, z);
            if let Some(mut hub) = child.get_resource_mut::<RegionHub<RegionId>>() {
                hub.spawn_region(region);
            }
            // Demo entities: 2 per region with Transform + PhysicsBody at
            // different heights so gravity is observable.
            for i in 0..2 {
                child.spawn((
                    EntityRegion(region),
                    Transform::new(
                        Position::new(x as f32 * 4.0, 80.0 + i as f32, z as f32 * 4.0),
                        Rotation::default(),
                    ),
                    PhysicsBody::default(),
                ));
            }
            regions.push(region);
        }
    }

    // Start one region thread per region sharing the child World.
    for region in regions {
        driver.start(&child, region, GAME_TICK_INTERVAL, region_tick);
    }
    log::info!(
        "{}",
        t_log!(
            "console.game.regions_started",
            count = driver.region_count(),
            ms = GAME_TICK_INTERVAL.as_millis()
        )
    );
}

/// Demo: every 300 ticks migrate the main world's first demo entity to
/// the nether world instance. Runs only with demo_systems=true.
fn demo_cross_world_transfer(
    mut hub: ResMut<WorldHub>,
    mut tick: Local<u32>,
    demo: Res<DemoSystems>,
) {
    if !demo.0 {
        return;
    }
    *tick += 1;
    if *tick % 300 != 0 {
        return;
    }
    let source = hub.get(&GameWorldId(0)).map(|game| game.world.clone());
    let target = hub.get(&GameWorldId(1)).map(|game| game.world.clone());
    let (Some(source), Some(target)) = (source, target) else {
        return;
    };
    let Some(entity) = source
        .entities_with_component::<EntityRegion<RegionId>>()
        .first()
        .copied()
    else {
        return;
    };
    if let Some(new_entity) = source.transfer_entity(&target, &entity) {
        log::info!(
            "{}",
            t_log!("console.game.demo_transfer", entity = format!("{new_entity:?}"))
        );
    }
}

/// Player region ownership: maintain the `EntityRegion` component by
/// position. Region size matches the region grid.
fn update_player_region(world: World, map: Res<GameWorldMap>) {
    const REGION_SIZE_CHUNKS: u32 = 8;
    for entity in world.entities_with_component::<MinecraftClient>() {
        let Some(movement) = world.get_component::<PlayerMovement>(&entity) else {
            continue;
        };
        let Some(world_id) = world.get_component::<MinecraftWorldId>(&entity) else {
            continue;
        };
        let Some(game_world_id) = map.find_by_world(world_id.as_ref()) else {
            continue;
        };
        let pos = {
            let state = movement.state.read();
            state.last_position
        };
        let Some(pos) = pos else {
            continue;
        };
        let region = RegionGrid::new(game_world_id, REGION_SIZE_CHUNKS)
            .region_of(pos.x.floor() as i32, pos.z.floor() as i32);
        // Region ownership (EntityRegion).
        match world.get_component::<EntityRegion<RegionId>>(&entity) {
            Some(er) => {
                if er.0 != region {
                    world.remove_component::<EntityRegion<RegionId>>(&entity);
                    world.add_component(&entity, EntityRegion(region));
                }
            }
            None => {
                world.add_component(&entity, EntityRegion(region));
            }
        }
        // Game-domain ownership (PlayerGameWorld).
        match world.get_component::<PlayerGameWorld>(&entity) {
            Some(pgw) => {
                if pgw.0 != game_world_id {
                    world.remove_component::<PlayerGameWorld>(&entity);
                    world.add_component(&entity, PlayerGameWorld(game_world_id));
                }
            }
            None => {
                world.add_component(&entity, PlayerGameWorld(game_world_id));
            }
        }
    }
}

/// Multi-world tick: advance per-instance `TickCounter`s, emit one
/// network intent per 40 ticks, ping main from other worlds per 100,
/// and demo a cross-world teleport per 500.
fn hub_tick(
    world: World,
    mut hub: ResMut<WorldHub>,
    mut ping_bus: ResMut<PingBus>,
    mut teleport_bus: ResMut<TeleportBus>,
    mut announcement_bus: ResMut<AnnouncementBus>,
    demo: Res<DemoSystems>,
) {
    for child in hub.running_mut() {
        let mut ticks = 0u64;
        if let Some(mut counter) = child.world.get_resource_mut::<TickCounter>() {
            counter.ticks += 1;
            ticks = counter.ticks;
        }
        // Branch B: outbound intents (fire-and-forget bypass).
        if !demo.0 {
            continue;
        }
        // Cross-world example: ping main from this world.
        if ticks % 100 == 0 && child.id != GameWorldId(0) {
            ping_bus.send(
                child.id,
                GameWorldId(0),
                CrossWorldPing {
                    origin: child.id,
                    from_tick: ticks,
                },
            );
        }
        // Every 400 ticks the nether announces to main.
        if ticks % 400 == 0 && child.id == GameWorldId(1) {
            announcement_bus.send(
                child.id,
                GameWorldId(0),
                WorldAnnouncement {
                    from: child.id,
                    message: format!("下界 tick {} 公告", ticks),
                },
            );
        }
        // Demo: every 500 ticks teleport the first online player to the nether.
        if ticks % 500 == 0 && child.id == GameWorldId(0) {
            if let Some(player) = world
                .entities_with_component::<MinecraftClient>()
                .first()
                .copied()
            {
                teleport_bus.send(
                    child.id,
                    GameWorldId(1),
                    TeleportRequest {
                        from: child.id,
                        to: GameWorldId(1),
                        entity: player,
                        x: 8.0,
                        y: 64.0,
                        z: 8.0,
                    },
                );
            }
        }
    }
}

/// Mount per-dimension GameWorld instances (Overworld=0 / Nether=1 /
/// End=2) and fill the `GameWorldMap`.
fn spawn_dimensions(world: World, mut hub: ResMut<WorldHub>, mut map: ResMut<GameWorldMap>) {
    let Some(manager) = world.get_resource::<MinecraftWorldManager>() else {
        return;
    };
    let mapping = [
        (WorldType::Overworld, 0u32),
        (WorldType::TheNether, 1),
        (WorldType::TheEnd, 2),
    ];
    for (world_type, id) in mapping {
        let worlds = manager.get_worlds_by_type(&world_type);
        let Some(minecraft_world) = worlds.first() else {
            continue;
        };
        let game_world_id = GameWorldId(id);
        if !hub.contains(&game_world_id) {
            hub.spawn_world(game_world_id);
        }
        // World instances share world data and region facilities (idempotent).
        if let Some(game_world) = hub.get(&game_world_id) {
            let child = game_world.world.clone();
            if child.get_resource::<MinecraftWorldManager>().is_none() {
                child.insert_resource((*manager).clone());
            }
            if child.get_resource::<RegionHub<RegionId>>().is_none() {
                child.insert_resource(RegionHub::<RegionId>::default());
            }
            if child.get_resource::<RegionPingBus>().is_none() {
                child.insert_resource(RegionPingBus::default());
            }
            if child.get_resource::<RegionCommands>().is_none() {
                // Region command set with region physics registered.
                let commands = RegionCommands::default();
                commands.register(Arc::new(RegionPhysicsCommand));
                child.insert_resource(commands);
            }
        }
        map.insert(
            game_world_id,
            GameWorldEntry {
                dimension: minecraft_world.world_data.get_dimension(),
                minecraft_world_id: minecraft_world.world_id.clone(),
            },
        );
    }
    log::info!(
        "{}",
        t_log!("console.game.dimensions", count = hub.count())
    );
}

/// Consume cross-world teleport requests (TeleportBus) and apply them:
/// update world ownership, position, and chunk subscription reset.
fn process_teleports(world: World, map: Res<GameWorldMap>, mut bus: ResMut<TeleportBus>) {
    let requests = bus.drain_outbox();
    if requests.is_empty() {
        return;
    }
    for event in requests {
        let request = event.payload;
        let Some(entry) = map.get(&request.to) else {
            log::debug!("teleport target world {} not mounted, dropping", request.to.0);
            continue;
        };
        // Target entity (currently a root-world player).
        if world
            .get_component::<MinecraftClient>(&request.entity)
            .is_none()
        {
            log::debug!("teleport target entity missing (root World), dropping");
            continue;
        }
        // 1) World ownership (detach first, avoiding stale rows).
        world.remove_component::<MinecraftWorldId>(&request.entity);
        world.add_component(&request.entity, entry.minecraft_world_id.clone());
        // 1.5) Game-domain ownership (PlayerGameWorld).
        world.remove_component::<PlayerGameWorld>(&request.entity);
        world.add_component(&request.entity, PlayerGameWorld(request.to));
        // 2) Teleport state machine: set teleport_position (inputs ignored
        //    during drain); land via complete_teleports once subscribed.
        let target = Position::new(request.x, request.y, request.z);
        let rotation = world
            .get_component::<Transform>(&request.entity)
            .map(|transform| transform.inner.read().rotation)
            .unwrap_or_default();
        if let Some(movement) = world.get_component::<PlayerMovement>(&request.entity) {
            let mut state = movement.state.write();
            state.teleport_position = Some(target);
            state.teleport_ticks = 0;
            state.force_position = None;
            state.input_buffer.clear();
            state.last_position = Some(target);
            state.last_rotation = rotation;
            state.first_move = false;
        }
        if let Some(client) = world.get_component::<MinecraftClient>(&request.entity) {
            client.data.write().position = MinecraftPosition::new(request.x, request.y, request.z);
        }
        if let Some(transform) = world.get_component::<Transform>(&request.entity) {
            let mut tf = transform.inner.write();
            tf.position = target;
            tf.velocity = sc_entity::motion::Velocity::default();
        }
        // 3) Full chunk subscription reset for immediate reorder in the
        //    new world; stale old-world rows would otherwise block it.
        if let Some(view) = world.get_component::<ChunkView>(&request.entity) {
            let radius = { view.read().radius };
            // Ordering barrier: the network layer raises it on dimension
            // changes; here only subscription generations reset.
            view.reset_subscription(
                entry.minecraft_world_id.clone(),
                entry.dimension,
                radius,
                sc_world::chunk::ChunkPosition::from_world(
                    request.x.floor() as i32,
                    request.z.floor() as i32,
                ),
            );
        }
        log::info!(
            "{}",
            t_log!(
                "console.game.teleport_request",
                world = request.to.0,
                x = request.x,
                y = request.y,
                z = request.z
            )
        );
    }
}

/// Cross-world announcement consumption (placeholder: logs for now).
fn process_announcements(world: World, mut bus: ResMut<AnnouncementBus>) {
    let announcements = bus.drain_outbox();
    for event in announcements {
        log::info!(
            "{}",
            t_log!(
                "console.game.announce",
                from = event.from.0,
                to = event.payload.from.0,
                message = event.payload.message
            )
        );
    }
}

/// Teleport completion: while `teleport_position` pends, land once the
/// target chunks subscribe, then clear state and send the correction.
///
/// TELEPORT is lossless: on budget exhaustion keep `teleport_position`
/// and retry next tick instead of dropping the correction.
fn complete_teleports(world: World, mut outbox: ResMut<NetworkOutbox>) {
    let mut faults = world.get_resource_mut::<PendingConnectionFaults>();
    let mut publisher = IntentPublisher::new(&mut outbox)
        .with_world(&world)
        .maybe_with_faults(faults.as_deref_mut());
    for entity in world.entities_with_component::<PlayerMovement>() {
        let Some(movement) = world.get_component::<PlayerMovement>(&entity) else {
            continue;
        };
        let Some(mc_id) = world.get_component::<MinecraftEntityId>(&entity) else {
            continue;
        };
        let (target, timed_out, rotation) = {
            let mut state = movement.state.write();
            let Some(target) = state.teleport_position else {
                continue;
            };
            state.teleport_ticks = state.teleport_ticks.saturating_add(1);
            (
                target,
                state.teleport_ticks >= TELEPORT_COMPLETE_TIMEOUT_TICKS,
                state.last_rotation,
            )
        };
        // Target chunks subscribed (no ChunkView transition counts as ready).
        let ready = world
            .get_component::<ChunkView>(&entity)
            .map(|view| {
                let data = view.read();
                let key = sc_world::storage::ChunkKey::new(
                    data.dimension,
                    sc_world::chunk::ChunkPosition::from_world(
                        target.x.floor() as i32,
                        target.z.floor() as i32,
                    ),
                );
                data.used_chunks.contains(&key)
            })
            .unwrap_or(true);
        if !ready && !timed_out {
            continue;
        }
        // Without correction budget, stay pending and retry next tick;
        // client correction wins over server landing.
        if !publisher.admits(IntentReliability::ReliableFact) {
            let mut state = movement.state.write();
            state.teleport_ticks = TELEPORT_COMPLETE_TIMEOUT_TICKS;
            log::warn!(
                "{}",
                t_log!(
                    "console.game.teleport_correction",
                    x = target.x,
                    y = target.y,
                    z = target.z
                )
            );
            continue;
        }
        // Land: clear state and write position.
        {
            let mut state = movement.state.write();
            state.teleport_position = None;
            state.teleport_ticks = 0;
            state.force_position = None;
            state.input_buffer.clear();
            state.first_move = true;
            state.last_position = Some(target);
        }
        if let Some(client) = world.get_component::<MinecraftClient>(&entity) {
            client.data.write().position = MinecraftPosition::new(target.x, target.y, target.z);
        }
        if let Some(transform) = world.get_component::<Transform>(&entity) {
            let mut tf = transform.inner.write();
            tf.position = target;
            tf.rotation = rotation;
            tf.velocity = sc_entity::motion::Velocity::default();
        }
        world.remove_component::<crate::interaction::BreakingState>(&entity);
        let base_offset = world
            .get_component::<PhysicsBody>(&entity)
            .map(|physics| physics.base_offset)
            .unwrap_or(1.62);
        // Notify the client (MovePlayer TELEPORT outbound intent).
        publisher.publish(NetworkIntent::MovePlayer {
            entity_id: mc_id.0,
            x: target.x,
            y: target.y + base_offset,
            z: target.z,
            yaw: rotation.yaw,
            pitch: rotation.pitch,
            head_yaw: rotation.head_yaw,
            on_ground: false,
            mode: PlayerMoveMode::Teleport,
        });
        log::info!(
            "{}",
            t_log!(
                "console.game.teleport_done",
                x = target.x,
                y = target.y,
                z = target.z
            )
        );
    }
}

/// Cross-world dispatch: deliver the bus outbox to target mailboxes.
fn dispatch_ping_events(world: World, mut hub: ResMut<WorldHub>) {
    let Some(mut bus) = world.get_resource_mut::<PingBus>() else {
        return;
    };
    let events = bus.drain_outbox();
    if events.is_empty() {
        return;
    }
    for event in events {
        if let Some(child) = hub.get_mut(&event.to) {
            match child.world.get_resource_mut::<PingMailbox>() {
                Some(mut mailbox) => mailbox.push(event),
                None => {
                    let mut mailbox = PingMailbox::default();
                    mailbox.push(event);
                    child.world.insert_resource(mailbox);
                }
            }
        } else {
            log::debug!("cross-world event target {} missing, dropping", event.to.0);
        }
    }
}
