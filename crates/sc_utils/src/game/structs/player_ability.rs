use std::collections::HashSet;

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub enum PlayerAbility {
    Build,
    Mine,
    DoorsAndSwitches,
    OpenContainers,
    AttackPlayers,
    AttackMobs,
    OperatorCommands,
    Teleportation,
    Invulnerable,
    Flying,
    Mayfly,
    Instabuild,
    Lightning,
    FlySpeed,
    WalkSpeed,
    Muted,
    WorldBuilder,
    NoClip,
    PrivilegedBuilder,
    VerticalFlySpeed,
}

impl PlayerAbility {
    pub const CONTROLLABLE_ABILITIES: &'static [PlayerAbility] = &[
        PlayerAbility::Build,
        PlayerAbility::Mine,
        PlayerAbility::DoorsAndSwitches,
        PlayerAbility::OpenContainers,
        PlayerAbility::AttackPlayers,
        PlayerAbility::AttackMobs,
        PlayerAbility::OperatorCommands,
        PlayerAbility::Teleportation,
    ];

    pub fn all() -> Vec<PlayerAbility> {
        let mut abilities = Vec::new();
        abilities.push(PlayerAbility::Build);
        abilities.push(PlayerAbility::Mine);
        abilities.push(PlayerAbility::DoorsAndSwitches);
        abilities.push(PlayerAbility::OpenContainers);
        abilities.push(PlayerAbility::AttackPlayers);
        abilities.push(PlayerAbility::AttackMobs);
        abilities.push(PlayerAbility::OperatorCommands);
        abilities.push(PlayerAbility::Teleportation);
        abilities.push(PlayerAbility::Invulnerable);
        abilities.push(PlayerAbility::Flying);
        abilities.push(PlayerAbility::Mayfly);
        abilities.push(PlayerAbility::Instabuild);
        abilities.push(PlayerAbility::Lightning);
        abilities.push(PlayerAbility::FlySpeed);
        abilities.push(PlayerAbility::WalkSpeed);
        abilities.push(PlayerAbility::Muted);
        abilities.push(PlayerAbility::WorldBuilder);
        abilities.push(PlayerAbility::NoClip);
        abilities.push(PlayerAbility::PrivilegedBuilder);
        abilities.push(PlayerAbility::VerticalFlySpeed);
        abilities
    }
}

#[derive(Clone, Debug)]
pub struct PlayerAbilityLayer {
    pub layer_type: AbilityLayerType,
    pub ability_set: HashSet<PlayerAbility>,
    pub ability_value: HashSet<PlayerAbility>,
    pub fly_speed: f32,
    pub vertical_fly_speed: f32,
    pub walk_speed: f32,
}

impl PlayerAbilityLayer {
    pub fn new(layer_type: AbilityLayerType) -> Self {
        Self {
            layer_type,
            ability_set: HashSet::new(),
            ability_value: HashSet::new(),
            fly_speed: 0.05,
            vertical_fly_speed: 1.0,
            walk_speed: 0.1,
        }
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum AbilityLayerType {
    Cache,
    Base,
    Spectator,
    Commands,
    Editor,
    LoadingScreen,
}

impl AbilityLayerType {
    pub fn index(&self) -> usize {
        match self {
            AbilityLayerType::Cache => 0,
            AbilityLayerType::Base => 1,
            AbilityLayerType::Spectator => 2,
            AbilityLayerType::Commands => 3,
            AbilityLayerType::Editor => 4,
            AbilityLayerType::LoadingScreen => 5,
        }
    }
}
