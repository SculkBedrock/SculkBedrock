use crate::app::plugin::Plugin;
use crate::app::App;
use crate::entity::EntityId;
use crate::params::component::Components;
use crate::params::resource::{Res, ResMut};
use crate::schedule::Startup;
use crate::world::World;
use sc_ecs_macros::{async_system, Component, Resource};

#[derive(Resource)]
pub struct TestResource {
    text: &'static str,
    id: Option<EntityId>,
}

#[derive(Component)]
pub struct TestComponent {
    test: &'static str,
}

pub struct TestPlugin;

impl Plugin for TestPlugin {
    fn build(&self, app: &App) {
        app.insert_resource(TestResource {
            text: "Resource Read Test",
            id: None,
        });
    }
}

#[test]
fn app_runs_startup_systems_with_plugins_and_resources() {
    App::new()
        .add_plugins(TestPlugin)
        .add_systems(Startup, (async_system_test, system_test, res_mut_test))
        .run();
}

#[test]
fn events_resource_id_distinct() {
    use crate::event::{Event, Events};
    use crate::resource::Resource;

    #[derive(Event, Clone, Debug)]
    #[allow(dead_code)]
    struct EvA {
        x: u32,
    }

    #[derive(Event, Clone, Debug)]
    #[allow(dead_code)]
    struct EvB {
        x: u32,
    }

    let a = <Events<EvA> as Resource>::resource_id();
    let b = <Events<EvB> as Resource>::resource_id();
    assert_ne!(
        a,
        b,
        "Events<EvA> and Events<EvB> must not share a ResourceId (type_name suffix missing)"
    );
}

#[async_system("block_on")]
async fn async_system_test(world: World, mut res: ResMut<TestResource>) {
    let id = world.spawn(TestComponent {
        test: "Component Test",
    });
    res.id = Some(id);
}

fn system_test(mut res: ResMut<TestResource>) {
    res.text = "Modify Resource Test";
}

fn res_mut_test(res: Res<TestResource>, components: Components<TestComponent>) {
    // The read conflicts with the async system's write, so the entity it
    // spawned must already be queryable with its component payload.
    let id = res.id.expect("the async startup system must set the entity id");
    let component = components
        .get(&id)
        .expect("the spawned entity must carry its component");
    assert_eq!(component.test, "Component Test");
}

#[test]
fn derived_component_static_name_and_query_count_use_index() {
    use crate::component::Component;
    use crate::params::query::Query;

    assert_eq!(TestComponent::name_static(), Some("TestComponent"));
    let world = World::new();
    let first = world.spawn(TestComponent { test: "first" });
    let query = Query::<TestComponent>::new(world.clone());
    assert_eq!(query.len(), 1);
    assert!(!query.is_empty());

    world.add_component(
        &first,
        TestComponent {
            test: "replacement",
        },
    );
    assert_eq!(world.component_count::<TestComponent>(), 1);
    let second = world.spawn(TestComponent { test: "second" });
    assert_eq!(query.len(), 2);

    world.remove_component::<TestComponent>(&first);
    assert_eq!(query.len(), 1);
    world.despawn(&second);
    assert!(query.is_empty());
}

#[test]
fn schedule_labels_match_across_module_copies() {
    use crate::intern::Interned;
    use crate::schedule::{ScheduleLabel, Update};
    use std::collections::HashMap;

    // Labels are keyed by type, not by value: one schedule slot per label
    // type even if a label carries data.
    #[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
    struct ProbeLabel(u8);

    // Simulates host vs cdylib-plugin copies of this crate: each module
    // instance leaks its own box (distinct heap addresses, distinct TypeId
    // namespaces) into its own interner. Identity must still hold by
    // stable type name, or plugin `add_systems` lands in an orphaned
    // schedule slot the host tick loop never visits.
    let host_leaked: &'static dyn ScheduleLabel = Box::leak(Box::new(ProbeLabel(5)));
    let plugin_leaked: &'static dyn ScheduleLabel = Box::leak(Box::new(ProbeLabel(5)));
    assert!(
        !std::ptr::addr_eq(
            std::ptr::from_ref::<dyn ScheduleLabel>(host_leaked).cast::<()>(),
            std::ptr::from_ref::<dyn ScheduleLabel>(plugin_leaked).cast::<()>(),
        ),
        "test premise: two independent leaks must have distinct addresses"
    );
    let host_key = Interned(host_leaked);
    let plugin_key = Interned(plugin_leaked);
    assert_eq!(host_key, plugin_key, "same-source labels must be equal");
    assert_eq!(
        host_leaked.label_name(),
        plugin_leaked.label_name(),
        "label names must be stable"
    );
    let mut map: HashMap<Interned<dyn ScheduleLabel>, u32> = HashMap::new();
    map.insert(host_key, 7);
    assert_eq!(
        map.get(&plugin_key),
        Some(&7),
        "plugin-interned key must hit the host schedule slot"
    );
    // Distinct label types must stay distinct.
    assert_ne!(host_leaked.label_name(), Update.label_name());
}
