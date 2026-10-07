use crate::world::client_data::MinecraftClientData;
use parking_lot::RwLock;
use sc_ecs::component::Component;
use sc_ecs::entity::EntityId;
use sc_ecs::world::World;
use std::sync::atomic::AtomicI32;
use std::sync::Arc;

#[derive(Component, Clone)]
pub struct MinecraftClient {
    pub entity_id: EntityId,
    pub world: World,
    pub data: Arc<RwLock<MinecraftClientData>>,
    in_air_ticks: Arc<AtomicI32>,
}

impl MinecraftClient {
    pub fn new(entity_id: EntityId, world: World, data: MinecraftClientData) -> Self {
        Self {
            entity_id,
            world,
            data: Arc::new(RwLock::new(data)),
            in_air_ticks: Arc::new(AtomicI32::new(0)),
        }
    }

    pub fn set_experience(&self, _exp: i32, _exp_level: i32) {}

    pub fn set_operator(&self, _op: bool) {}

    pub fn reset_in_air_ticks(&self) {
        self.in_air_ticks
            .store(0, std::sync::atomic::Ordering::Relaxed);
    }
}
