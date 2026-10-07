use parking_lot::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use sc_ecs::entity::EntityId;
use sc_ecs::resource::Resource;
use sc_ecs::world::World;
use std::collections::HashMap;
use std::sync::OnceLock;
use uuid::Uuid;

static SERVER: OnceLock<RwLock<Server>> = OnceLock::new();

#[derive(Resource, Clone)]
pub struct Server {
    pub server_version: String,
    pub world: World,
    clients: HashMap<Uuid, EntityId>,
}

impl Server {
    pub fn init(server: Server) {
        let _ = SERVER.set(RwLock::new(server));
    }

    pub fn default(world: World) -> Self {
        Self {
            server_version: "1.0.0ALPHA(fjord)".to_string(),
            world,
            clients: HashMap::new(),
        }
    }

    pub fn global() -> Option<RwLockReadGuard<'static, Server>> {
        SERVER.get().map(|server| server.read())
    }

    pub fn global_mut() -> Option<RwLockWriteGuard<'static, Server>> {
        SERVER.get().map(|server| server.write())
    }

    pub fn push_client(&mut self, uuid: Uuid, entity: EntityId) {
        self.clients.insert(uuid, entity);
    }

    /// Remove a client from the online list. Must be called when a player
    /// disconnects to prevent the `clients` HashMap from growing unbounded
    /// over many join/leave cycles. Also shrinks the HashMap to release
    /// excess capacity.
    pub fn remove_client(&mut self, uuid: Uuid) -> Option<EntityId> {
        let result = self.clients.remove(&uuid);
        self.clients.shrink_to_fit();
        result
    }

    pub fn get_online_clients_mut(&mut self) -> &mut HashMap<Uuid, EntityId> {
        &mut self.clients
    }

    pub fn get_online_clients(&self) -> &HashMap<Uuid, EntityId> {
        &self.clients
    }

    pub fn get_client(&self, uuid: Uuid) -> Option<EntityId> {
        self.clients.get(&uuid).cloned()
    }
}
