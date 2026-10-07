use crate::chunk_executor::WorldChunkExecutor;
use crate::chunk_view::{ChunkSendSettings, LevelChunkCache};
use crate::manager::{MinecraftWorldId, MinecraftWorldManager};
use crate::world_loader::dir_loader::WorldDirectoryLoader;
use crate::world_loader::WorldLoaderTrait;
use crate::writeback::ChunkWritebackExecutor;
use chrono::Local;
use log::{error, info, trace};
use sc_ecs::app::plugin::Plugin;
use sc_ecs::app::App;
use sc_ecs::params::event::EventReader;
use sc_ecs::params::resource::Res;
use sc_ecs::schedule::{Last, Update};
use sc_ecs::world::World;
use sc_log::t_log;
use sc_utils::event::{SCExit, SCExitReason, SCExitType};
use sc_utils::game::structs::server_properties::ServerProperties;
use sc_utils::schedule::SCPreLoad;
use sc_utils::world::r#type::WorldType;
use std::env;
use std::io::{Error, ErrorKind};
use std::time::Duration;

pub mod biome;
pub mod block_dictionary;
pub mod chunk;
pub mod chunk_executor;
pub mod chunk_view;
pub mod data_reader;
pub mod leveldb;
pub mod manager;
pub mod storage;
pub mod world;
pub mod world_loader;
pub mod writeback;

pub struct SCWorldPlugin;

impl Plugin for SCWorldPlugin {
    fn build(&self, app: &App) {
        app.add_systems(SCPreLoad, load_world)
            .add_systems(Update, process_chunk_writebacks)
            // Shutdown flush: stop the chunk executor, then write dirty chunks.
            // Registered at Last; this plugin registers before SCGamePlugin,
            // so this system runs before the exit system and flush completes.
            .add_systems(Last, on_exit_flush_chunks);
    }
}

/// Shutdown flush: on [`SCExit`], stop the [`WorldChunkExecutor`] first, then
/// write all dirty chunks back to storage.
fn on_exit_flush_chunks(world: World, mut reader: EventReader<SCExit>) {
    if reader.read().next().is_none() {
        return;
    }
    // 1) Stop the chunk executor (idempotent).
    if let Some(executor) = world.get_resource::<WorldChunkExecutor>() {
        executor.shutdown();
        info!("{}", t_log!("console.world.executor_stopped"));
    }
    if let Some(writebacks) = world
        .get_resource::<ChunkWritebackExecutor>()
        .map(|resource| (*resource).clone())
    {
        // Bounded wait: never join forever on a wedged disk without reporting.
        let shutdown = writebacks.shutdown();
        let mut completed = 0usize;
        loop {
            let report = writebacks.poll_completed(usize::MAX);
            completed += report.completed;
            if report.completed == 0 {
                break;
            }
        }
        let unconfirmed = writebacks.in_flight_jobs();
        if shutdown.is_complete() && unconfirmed == 0 {
            info!(
                "{}",
                t_log!("console.world.writeback_stopped", completed = completed)
            );
        } else {
            // Report unconfirmed data explicitly instead of silently claiming safety.
            error!(
                "{}",
                t_log!(
                    "console.world.writeback_unconfirmed",
                    completed = completed,
                    timed_out = shutdown.timed_out,
                    unconfirmed = shutdown.unconfirmed_jobs,
                    unconfirmed_bytes = shutdown.unconfirmed_bytes,
                    at_timeout = shutdown.unconfirmed_jobs,
                    bytes = shutdown.unconfirmed_bytes
                )
            );
        }
    }
    // 2) Flush dirty chunks of all worlds; report success only when the dirty set drains.
    let providers = {
        let Some(manager) = world.get_resource::<MinecraftWorldManager>() else {
            error!("{}", t_log!("console.world.exit_flush_no_manager"));
            return;
        };
        manager
            .worlds()
            .map(|minecraft_world| minecraft_world.chunk_provider.clone())
            .collect::<Vec<_>>()
    };
    let mut attempted = 0usize;
    let mut saved = 0usize;
    let mut failed = 0usize;
    let mut changed_during_save = 0usize;
    let mut remaining_dirty = Some(0usize);
    let mut scan_failed = false;
    let mut key_samples_truncated = false;
    for provider in providers {
        let report = provider.flush_dirty_report();
        attempted += report.attempted;
        saved += report.saved;
        failed += report.failed_count;
        changed_during_save += report.changed_during_save_count;
        scan_failed |= report.scan_failed;
        key_samples_truncated |= report.key_samples_truncated;
        remaining_dirty = match (remaining_dirty, report.remaining_dirty) {
            (Some(total), Some(world_dirty)) => Some(total.saturating_add(world_dirty)),
            _ => None,
        };
    }
    if failed == 0 && changed_during_save == 0 && remaining_dirty == Some(0) && !scan_failed {
        info!(
            "{}",
            t_log!(
                "console.world.exit_flush_done",
                saved = saved,
                attempted = attempted
            )
        );
    } else {
        error!(
            "{}",
            t_log!(
                "console.world.exit_flush_unconfirmed",
                attempted = attempted,
                saved = saved,
                failed = failed,
                changed = changed_during_save,
                remaining = format!("{remaining_dirty:?}"),
                scan = scan_failed,
                keys = key_samples_truncated
            )
        );
    }
}

/// Poll bounded SaveAcks and admit a fair, bounded number of new dirty
/// snapshots. This system only clones world/provider handles under the ECS
/// resource guard; storage work runs on the dedicated writeback workers.
fn process_chunk_writebacks(world: World) {
    const MAX_COMPLETIONS_PER_TICK: usize = 32;
    const MAX_SUBMISSIONS_PER_TICK: usize = 8;

    let Some(writebacks) = world
        .get_resource::<ChunkWritebackExecutor>()
        .map(|resource| (*resource).clone())
    else {
        return;
    };
    let completed = writebacks.poll_completed(MAX_COMPLETIONS_PER_TICK);
    if completed.failed > 0 || completed.superseded > 0 {
        log::warn!(
            "{}",
            t_log!(
                "console.world.dirty_ack",
                saved = completed.saved,
                failed = completed.failed,
                superseded = completed.superseded
            )
        );
    }
    let aged = writebacks.overdue_report(Duration::from_secs(10), 8);
    if aged.newly_overdue > 0 {
        log::warn!(
            "{}",
            t_log!(
                "console.world.dirty_overdue",
                new = aged.newly_overdue,
                total = aged.overdue,
                oldest = format!("{:?}", aged.oldest_age),
                keys = format!("{:?}", aged.key_samples)
            )
        );
    }

    let worlds = {
        let Some(manager) = world.get_resource::<MinecraftWorldManager>() else {
            return;
        };
        manager
            .worlds()
            .map(|minecraft_world| {
                (
                    minecraft_world.world_id.clone(),
                    minecraft_world.chunk_provider.clone(),
                )
            })
            .collect::<Vec<(MinecraftWorldId, crate::storage::WorldChunkProvider)>>()
    };
    let submitted = writebacks.submit_worlds_round_robin(&worlds, MAX_SUBMISSIONS_PER_TICK);
    if submitted > 0 {
        trace!("world >> admitted {submitted} dirty chunk writeback(s)");
    }
}

fn load_world(world: World, server_properties: Res<ServerProperties>) {
    //world
    let start = Local::now().timestamp_millis();
    let mut current_dir = match env::current_dir() {
        Ok(current_dir) => current_dir,
        Err(error) => {
            error!("{}", t_log!("console.world.cwd_fail", error = error));
            world.send_event(SCExit::new(
                SCExitReason::Error(Box::new(error)),
                SCExitType::Shutdown,
            ));
            return;
        }
    };
    current_dir.push("worlds");
    let loader = WorldDirectoryLoader::new(current_dir);
    let worlds = loader.get_worlds();
    let mut world_manager = MinecraftWorldManager::new();

    let overworld_name = server_properties.overworld_name.clone();
    let the_nether_name = server_properties.the_nether_name.clone();
    let the_end_name = server_properties.the_end_name.clone();

    let mut has_overworld = false;

    for mut world in worlds.clone() {
        let world_name = world.world_name.clone();
        trace!("World Name(FILE) >> {}", &world.world_name);
        trace!("World Name(NBT) >> {}", &world_name);
        let ty = if world_name == overworld_name {
            has_overworld = true;
            WorldType::Overworld
        } else if world_name == the_nether_name {
            WorldType::TheNether
        } else if world_name == the_end_name {
            WorldType::TheEnd
        } else {
            WorldType::Custom(Box::new(WorldType::Overworld))
        };
        world.world_data.world_type = Some(ty.clone());
        trace!("World Type: {:?}", &ty);
        world_manager.push_world(ty, world);
    }

    // Shut the server down when no overworld is found.
    if !has_overworld {
        world.send_event(SCExit::new(
            SCExitReason::Error(Box::new(Error::new(
                ErrorKind::NotFound,
                "No overworld found",
            ))),
            SCExitType::Shutdown,
        ));
        return;
    }

    let executor = match WorldChunkExecutor::new(WorldChunkExecutor::default_worker_count()) {
        Ok(executor) => executor,
        Err(error) => {
            error!(
                "{}",
                t_log!("console.world.chunk_workers_fail", error = error)
            );
            world.send_event(SCExit::new(
                SCExitReason::Error(Box::new(error)),
                SCExitType::Shutdown,
            ));
            return;
        }
    };
    let writebacks =
        match ChunkWritebackExecutor::new(ChunkWritebackExecutor::default_worker_count()) {
            Ok(writebacks) => writebacks,
            Err(error) => {
                executor.shutdown();
                error!(
                    "{}",
                    t_log!("console.world.writeback_workers_fail", error = error)
                );
                world.send_event(SCExit::new(
                    SCExitReason::Error(Box::new(error)),
                    SCExitType::Shutdown,
                ));
                return;
            }
        };

    world.insert_resource(world_manager);
    // Chunk pipeline config ([chunk] section, falls back to defaults when absent).
    world.insert_resource(ChunkSendSettings::from_properties(
        server_properties.view_distance,
        server_properties.chunks_per_tick,
        server_properties.spawn_threshold,
    ));
    // Chunk payload cache (consumed by order_chunks/send_next_chunk).
    world.insert_resource(LevelChunkCache::default());
    // Chunk load/generate executor: LevelDB I/O + worldgen run off Tokio,
    // on a dedicated bounded worker pool with same-key dedup; send_chunks_batch only awaits receipts.
    world.insert_resource(executor);
    world.insert_resource(writebacks);
    let end = Local::now().timestamp_millis();

    info!(
        "{}",
        t_log!("console.level.load", count = worlds.len(), ms = end - start)
    );
}
