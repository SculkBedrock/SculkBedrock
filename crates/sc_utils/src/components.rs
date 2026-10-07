use sc_ecs::component::Component;
use serde::{Deserialize, Serialize};

#[derive(Component, Serialize, Deserialize, Debug, Clone)]
pub struct DisplayName(pub String);

#[derive(Component, Serialize, Deserialize, Debug, Clone)]
pub struct RuntimeID(pub i16);

#[derive(Component, Serialize, Deserialize, Debug, Clone)]
pub struct NetworkID(pub i32);
