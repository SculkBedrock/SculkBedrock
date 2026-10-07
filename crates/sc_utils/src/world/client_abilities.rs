use crate::nbt::deserialize_nbt_bool;
use serde::Deserialize;
use serde_inline_default::serde_inline_default;

/// Represents the "abilities" compound in world.dat NBT.
/// Stores player permission ability flags and movement speeds.
#[serde_inline_default]
#[derive(Deserialize, Debug, Clone)]
pub struct ClientAbilities {
    #[serde_inline_default(false)]
    #[serde(alias = "build")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub build: bool,
    #[serde_inline_default(false)]
    #[serde(alias = "mine")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub mine: bool,
    #[serde_inline_default(false)]
    #[serde(alias = "doorsandswitches")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub doors_and_switches: bool,
    #[serde_inline_default(true)]
    #[serde(alias = "opencontainers")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub open_containers: bool,
    #[serde_inline_default(true)]
    #[serde(alias = "attackplayers")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub attack_players: bool,
    #[serde_inline_default(true)]
    #[serde(alias = "attackmobs")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub attack_mobs: bool,
    #[serde_inline_default(false)]
    #[serde(alias = "teleport")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub teleport: bool,
    #[serde_inline_default(false)]
    #[serde(alias = "invulnerable")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub invulnerable: bool,
    #[serde_inline_default(false)]
    #[serde(alias = "flying")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub flying: bool,
    #[serde_inline_default(false)]
    #[serde(alias = "mayfly")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub may_fly: bool,
    #[serde_inline_default(false)]
    #[serde(alias = "instabuild")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub insta_build: bool,
    #[serde_inline_default(false)]
    #[serde(alias = "lightning")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub lightning: bool,
    #[serde_inline_default(false)]
    #[serde(alias = "op")]
    #[serde(deserialize_with = "deserialize_nbt_bool")]
    pub op: bool,
    #[serde_inline_default(0.05)]
    #[serde(alias = "flySpeed")]
    pub fly_speed: f32,
    #[serde_inline_default(0.1)]
    #[serde(alias = "walkSpeed")]
    pub walk_speed: f32,
    #[serde_inline_default(1.0)]
    #[serde(alias = "verticalFlySpeed")]
    pub vertical_fly_speed: f32,
}

impl Default for ClientAbilities {
    fn default() -> Self {
        Self {
            build: false,
            mine: false,
            doors_and_switches: false,
            open_containers: true,
            attack_players: true,
            attack_mobs: true,
            teleport: false,
            invulnerable: false,
            flying: false,
            may_fly: false,
            insta_build: false,
            lightning: false,
            op: false,
            fly_speed: 0.05,
            walk_speed: 0.1,
            vertical_fly_speed: 1.0,
        }
    }
}

impl ClientAbilities {
    pub fn new() -> Self {
        Self::default()
    }
}
