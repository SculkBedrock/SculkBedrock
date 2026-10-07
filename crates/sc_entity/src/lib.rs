use crate::manager::MinecraftEntitiesManager;
use sc_ecs::app::plugin::Plugin;
use sc_ecs::app::App;
use sc_ecs::bundle::Bundle;
use sc_ecs::component::Component;
use sc_ecs::entity::EntityId;
use sc_ecs::world::World;
use sc_packloader::version_control::runtime::MinecraftRuntimeManager;

mod effect;
pub mod manager;
pub mod motion;
pub mod player;
pub mod viewer;

#[derive(Component, Clone, Copy, Debug)]
pub struct MinecraftEntityId(pub u64);

pub struct SCEntityPlugin;

impl Plugin for SCEntityPlugin {
    fn build(&self, app: &App) {
        app.insert_resource(MinecraftEntitiesManager::new());
    }
}

pub trait SpawnEntity {
    fn spawn_entity<B: Bundle>(&self, identifier: &str, bundle: B) -> EntityId;
    fn insert_entity(&self, entity_id: &EntityId, identifier: &str);
    fn insert_entity_with<B: Bundle>(&self, entity_id: &EntityId, identifier: &str, bundle: B);
}

impl SpawnEntity for World {
    fn spawn_entity<B: Bundle>(&self, identifier: &str, bundle: B) -> EntityId {
        let entity_id = self.spawn(bundle);
        let Some(runtime_manager) = self.get_resource::<MinecraftRuntimeManager>() else {
            eprintln!("entity spawn skipped: MinecraftRuntimeManager is not registered");
            return entity_id;
        };
        let Some(spawner) = runtime_manager.get_entity_spawner_by_ident(identifier) else {
            eprintln!("entity spawn skipped: unknown entity identifier `{identifier}`");
            return entity_id;
        };
        spawner.insert(self, &entity_id);

        let Some(mut entity_manager) = self.get_resource_mut::<MinecraftEntitiesManager>() else {
            eprintln!("entity spawn skipped: MinecraftEntitiesManager is not registered");
            return entity_id;
        };
        entity_manager.push_entity(self, &entity_id);
        entity_id
    }

    fn insert_entity(&self, entity_id: &EntityId, identifier: &str) {
        let Some(runtime_manager) = self.get_resource::<MinecraftRuntimeManager>() else {
            eprintln!("entity insert skipped: MinecraftRuntimeManager is not registered");
            return;
        };
        let Some(spawner) = runtime_manager.get_entity_spawner_by_ident(identifier) else {
            eprintln!("entity insert skipped: unknown entity identifier `{identifier}`");
            return;
        };
        spawner.insert(self, entity_id);

        let Some(mut entity_manager) = self.get_resource_mut::<MinecraftEntitiesManager>() else {
            eprintln!("entity insert skipped: MinecraftEntitiesManager is not registered");
            return;
        };
        entity_manager.push_entity(self, entity_id);
    }

    fn insert_entity_with<B: Bundle>(&self, entity_id: &EntityId, identifier: &str, bundle: B) {
        let Some(runtime_manager) = self.get_resource::<MinecraftRuntimeManager>() else {
            eprintln!("entity insert skipped: MinecraftRuntimeManager is not registered");
            return;
        };
        let Some(spawner) = runtime_manager.get_entity_spawner_by_ident(identifier) else {
            eprintln!("entity insert skipped: unknown entity identifier `{identifier}`");
            return;
        };
        spawner.insert(self, entity_id);
        self.add_component(entity_id, bundle);

        let Some(mut entity_manager) = self.get_resource_mut::<MinecraftEntitiesManager>() else {
            eprintln!("entity insert skipped: MinecraftEntitiesManager is not registered");
            return;
        };
        entity_manager.push_entity(self, entity_id);
    }
}
