use sc_ecs::app_manager::AppLabel;

#[derive(AppLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct MainApp;
#[derive(AppLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct GameLogicApp;
