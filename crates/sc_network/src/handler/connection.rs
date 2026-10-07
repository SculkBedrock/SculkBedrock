use crate::events::connection::{AcceptConnection, DropConnection};
use crate::events::{SCGameEvents, SCNetworkEvents};
use crate::player_connection::{PlayerConnection, PlayerConnectionError};
use crate::protocol::server::misc::{PlayerList, PlayerListEntry};
use crate::protocol::server::movement::RemoveEntity;
use crate::protocol::MinecraftPackets;
use crate::utils::ConnectionThreadManager;
use log::debug;
use sc_ecs::app::plugin::Plugin;
use sc_ecs::app::App;
use sc_ecs::async_manager::SCECSAsync;
use sc_ecs::entity::EntityId;
use sc_ecs::event::EnumEvents;
use sc_ecs::params::event::{EventReader, EventReaderMut};
use sc_ecs::params::resource::ResMut;
use sc_ecs::world::World;
use sc_entity::manager::MinecraftEntitiesManager;
use sc_entity::MinecraftEntityId;
use sc_eventbus::events::SCEnumEvents;
use sc_log::{t, t_log};
use sc_raknet::utils::to_address_token;
use sc_utils::game::structs::server::Server;
use sc_utils::schedule::SCConnectionUpdate;
use sc_world::manager::MinecraftWorldId;
use std::time::SystemTime;

pub struct SCConnectionHandlerPlugin;

impl Plugin for SCConnectionHandlerPlugin {
    fn build(&self, app: &App) {
        SCNetworkEvents::add_events(app);
        MinecraftPackets::add_events(app);
        SCGameEvents::add_events(app);
        app.insert_resource(ConnectionThreadManager::new())
            .add_systems(SCConnectionUpdate, (accept_connection, drop_connection))
            .add_systems(sc_ecs::schedule::Last, sweep_connection_threads);
    }
}

/// Reaps finished task handles every tick to bound handle-set growth (see
/// [`ConnectionThreadManager::sweep_finished`]).
fn sweep_connection_threads(mut manager: ResMut<ConnectionThreadManager>) {
    manager.sweep_finished();
}

fn accept_connection(
    world: World,
    mut event_reader: EventReaderMut<AcceptConnection>,
    mut manager: ResMut<ConnectionThreadManager>,
) {
    for event in event_reader.read() {
        if let Some(connection) = event.connection.take() {
            let (entity, location) = world.pre_spawn();
            let connection = PlayerConnection::new(connection, world.clone(), entity);
            world.spawn_with_location(connection, location);
            manager.insert(
                entity,
                SCECSAsync::runtime().spawn(receive_packet(world.clone(), entity)),
            );
        }
    }
}

fn drop_connection(
    world: World,
    mut event_reader: EventReader<DropConnection>,
    mut manager: ResMut<ConnectionThreadManager>,
    mut entity_manager: ResMut<MinecraftEntitiesManager>,
) {
    for event in event_reader.read() {
        // Gather connection info and clean up external registries BEFORE
        // despawning — world.despawn() removes all components, making them
        // inaccessible afterwards.

        let address_str = world
            .get_component::<PlayerConnection>(&event.entity)
            .map_or("Unknown".to_string(), |conn| {
                to_address_token(conn.connection.address)
            });

        // 1. Remove from MinecraftEntitiesManager (leaked: push_entity was
        //    called during spawn but remove was never called on disconnect).
        if let Some(mc_entity_id) = world.get_component::<MinecraftEntityId>(&event.entity) {
            entity_manager.remove_entity(mc_entity_id.0);
        }

        // 2. Remove from Server's online client list (future-proofing: even
        //    though push_client is not yet called, when it is added this
        //    cleanup will prevent the HashMap from growing unbounded).
        if let Some(connection) = world.get_component::<PlayerConnection>(&event.entity) {
            let data = connection.get_data();
            if !data.uuid.is_nil() {
                if let Some(mut server) = Server::global_mut() {
                    server.remove_client(data.uuid);
                }
            }
        }

        // Broadcasts PlayerList REMOVE + RemoveEntity to remaining same-world players
        // (identity values are captured first: components are unreadable after despawn).
        let quit_broadcast = (|| {
            let connection = world.get_component::<PlayerConnection>(&event.entity)?;
            let data = connection.get_data();
            let mc = world.get_component::<MinecraftEntityId>(&event.entity)?;
            let world_id = world.get_component::<MinecraftWorldId>(&event.entity)?;
            Some((data.uuid, mc.0, world_id.as_ref().clone()))
        })();
        if let Some((uuid, runtime_id, world_id)) = quit_broadcast {
            let world = world.clone();
            // event is loop-local, so copy the entity (Copy) into the closure to avoid a dangling borrow.
            let leave_entity = event.entity;
            SCECSAsync::runtime().spawn(async move {
                let others = crate::handler::player::same_world_players(
                    &world,
                    &world_id,
                    Some(&leave_entity),
                );
                for other in others {
                    let Some(other_connection) = world.get_component::<PlayerConnection>(&other)
                    else {
                        continue;
                    };
                    let _ = other_connection
                        .send_packet(
                            PlayerList {
                                list_type: PlayerList::TYPE_REMOVE,
                                entries: vec![PlayerListEntry {
                                    uuid,
                                    ..Default::default()
                                }],
                            },
                            true,
                        )
                        .await;
                    let _ = other_connection
                        .send_packet(
                            RemoveEntity {
                                entity_id: runtime_id,
                            },
                            true,
                        )
                        .await;
                }
            });
        }

        debug!("Connection({}) >> Dropped", address_str);

        // 3. Abort all per-entity handler tasks BEFORE despawning. These tasks
        //    (receive_packet, network handlers, player handlers) hold World
        //    clones and may access the entity's components. If we despawn first,
        //    the tasks could access freed component data (use-after-free).
        //    Aborting first schedules the task futures for asynchronous drop,
        //    releasing their World clones and Arc references sooner.
        manager.drop_thread(&event.entity);

        // 4. Clear per-player internal state (resource chunk requests, data,
        //    encryption) to release memory immediately rather than waiting
        //    for the Arc-shared state to be dropped when the component is
        //    despawned below.
        if let Some(connection) = world.get_component::<PlayerConnection>(&event.entity) {
            connection.cleanup();
        }

        // 5. Despawn the entity from the ECS world (removes all components
        //    and their associated memory). This drops PlayerConnection, which
        //    drops Connection (already cleared via close()), which drops the
        //    remaining Arc references to SendQueue/RecvQueue.
        world.despawn(&event.entity);
    }
}

pub(crate) async fn receive_packet(world: World, entity: EntityId) {
    let Some(connection) = world
        .get_component::<PlayerConnection>(&entity)
        .map(|connection| connection.clone())
    else {
        log::warn!("{}", t_log!("console.connection.stopped", entity = entity));
        return;
    };
    loop {
        let result = connection.recv().await;
        match result {
            Ok(packets) => {
                for packet in packets {
                    if !crate::handler::crafting::enqueue_ordered_crafting(&world, entity, &packet)
                    {
                        let _ = connection.disconnect("Crafting input overflow", true).await;
                        return;
                    }
                    if !crate::handler::interaction::enqueue_ordered_mining(&world, entity, &packet)
                    {
                        log::warn!(
                            "{}",
                            t_log!("console.connection.inbox_full", entity = entity)
                        );
                        let _ = connection
                            .disconnect("Interaction input overflow", true)
                            .await;
                        return;
                    }
                    packet.send_event(
                        &world,
                        entity,
                        SystemTime::now()
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .map(|duration| duration.as_millis())
                            .unwrap_or(0),
                    );
                }
            }
            Err(PlayerConnectionError::RecvError(_)) => {
                debug!(
                    "Connection({}) >> Closed",
                    to_address_token(connection.connection.address)
                );
                let _ = connection
                    .disconnect(
                        &t!("console.connection.undefined_redstone"),
                        true,
                    )
                    .await;
                return;
            }
            Err(error) => {
                // For non-RecvError (EncryptionError, SendQueueError, etc.),
                // the underlying connection is likely broken. Retrying in a
                // tight loop wastes CPU and permanently leaks memory because
                // this task never exits — drop_thread is only called from
                // drop_connection, which only fires after disconnect(), which
                // only happens in the RecvError branch above.
                //
                debug!(
                    "Connection({}) >> Recoverable packet error: {}",
                    to_address_token(connection.connection.address),
                    error
                );
                // Brief backoff before retry to avoid CPU spin
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}
