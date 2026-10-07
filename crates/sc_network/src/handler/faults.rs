//! Consumer of connection-level fault isolation (network domain).
//!
//! The game domain registers the terminal "this player can no longer stay
//! consistent" state via [`PendingConnectionFaults`]; this module consumes a batch
//! of requests every tick and actually disconnects those connections, so faulty
//!
//! Ordering and budget:
//! - At most [`MAX_FAULTS_PER_TICK`] requests per tick, avoiding a burst of
//!   disconnects in one tick (each disconnect `sleep(5s)`s for the reliable window);
//! - leftovers after `take_all()` are **returned to the queue**, never silently dropped;
//! - requests for already-gone entities only count (the request is moot), not failures.

use sc_ecs::async_manager::SCECSAsync;
use sc_ecs::world::World;
use sc_entity::MinecraftEntityId;
use sc_game::net_faults::{ConnectionFault, PendingConnectionFaults};

use crate::player_connection::PlayerConnection;
use sc_log::t_log;

/// Maximum isolations executed per tick.
const MAX_FAULTS_PER_TICK: usize = 8;

/// Consumes connection-isolation requests and disconnects those connections.
///
/// ECS systems must return `()`, so the per-tick count is only logged; the
/// public helper below is what tests and diagnostics use.
pub fn disconnect_faulted_connections(world: World) {
    let disconnected = take_faults_and_isolate(world);
    if disconnected > 0 {
        log::info!("{}", t_log!("console.fault.isolated", count = disconnected));
    }
}

/// Consume isolation requests, isolating up to [`MAX_FAULTS_PER_TICK`] of them.
///
/// Returns the number of connections this call actually started closing.
pub fn take_faults_and_isolate(world: World) -> usize {
    let Some(mut faults) = world.get_resource_mut::<PendingConnectionFaults>() else {
        return 0;
    };
    if faults.pending() == 0 {
        return 0;
    }
    let requested = faults.take_all();
    let mut processed = 0usize;
    let mut deferred = Vec::new();
    let mut disconnected = 0usize;
    for fault in requested {
        if processed >= MAX_FAULTS_PER_TICK {
            deferred.push(fault);
            continue;
        }
        processed += 1;
        if isolate_fault(&world, fault) {
            disconnected += 1;
        }
    }
    // Over-budget requests carry to later ticks: isolation must not be swallowed by the per-tick cap.
    faults.requeue_all(deferred);
    if disconnected > 0 {
        log::warn!(
            "{}",
            t_log!(
                "console.fault.isolated_tick",
                count = disconnected,
                pending = faults.pending()
            )
        );
    }
    disconnected
}

/// Terminate one connection for an unrecoverable terminal state.
fn isolate_fault(world: &World, fault: ConnectionFault) -> bool {
    let entity = world
        .entities_with_component::<PlayerConnection>()
        .into_iter()
        .find(|entity| {
            world
                .get_component::<MinecraftEntityId>(entity)
                .is_some_and(|id| id.0 == fault.runtime_id)
        });
    let Some(entity) = entity else {
        // The player already disconnected: the request has no target left.
        return false;
    };
    let Some(connection) = world.get_component::<PlayerConnection>(&entity) else {
        return false;
    };
    let connection = connection.clone();
    let reason = t_log!("console.fault.desync", reason = fault.reason).into_owned();
    SCECSAsync::runtime().spawn(async move {
        let _ = connection.disconnect(&reason, false).await;
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_registry_is_a_no_op() {
        let world = World::new();
        assert_eq!(take_faults_and_isolate(world.clone()), 0);
    }

    #[test]
    fn an_empty_registry_is_a_no_op() {
        let world = World::new();
        world.insert_resource(PendingConnectionFaults::default());
        assert_eq!(take_faults_and_isolate(world.clone()), 0);
        assert_eq!(
            world
                .get_resource::<PendingConnectionFaults>()
                .expect("registry")
                .pending(),
            0
        );
    }

    #[test]
    fn requests_for_missing_entities_are_counted_without_panicking() {
        let world = World::new();
        let mut faults = PendingConnectionFaults::with_limits(4);
        faults.request(4_242, "lifecycle");
        faults.request(4_243, "lifecycle");
        world.insert_resource(faults);

        // No matching connections exist, so nothing is isolated, but the queue is
        // drained rather than growing without bound.
        assert_eq!(take_faults_and_isolate(world.clone()), 0);
        assert_eq!(
            world
                .get_resource::<PendingConnectionFaults>()
                .expect("registry")
                .pending(),
            0
        );
    }
}
