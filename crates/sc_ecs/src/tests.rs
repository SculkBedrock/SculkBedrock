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
fn test() {
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
    struct EvA {
        x: u32,
    }

    #[derive(Event, Clone, Debug)]
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
    println!("Async System Test");
    let id = world.spawn(TestComponent {
        test: "Component Test",
    });
    res.id = Some(id);
}

fn system_test(mut res: ResMut<TestResource>) {
    println!("{}", res.text);
    res.text = "Modify Resource Test";
}

fn res_mut_test(res: Res<TestResource>, components: Components<TestComponent>) {
    println!("{}", res.text);
    println!("{}", components.get(&res.id.unwrap()).unwrap().test)
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
