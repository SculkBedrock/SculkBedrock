use crate::MinecraftEntityId;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use sc_ecs::component::Component;
use sc_ecs::entity::EntityId;
use sc_ecs::world::World;
use sc_nbt::compound::CompoundNbt;
use sc_utils::game::client::MinecraftClient;
use sc_utils::game::structs::permission::{CommandPermission, PlayerPermission};
use sc_utils::game::structs::player_ability::PlayerAbility;
use sc_utils::world::client_data::MinecraftClientData;

#[derive(Eq, Hash, PartialEq, Debug)]
pub enum AdventureSettingsType {
    WorldImmutable,
    NoPVM,
    NoMVP,
    ShowNameTags,
    AutoJump,
    AllowFlight,
    NoClip,
    WorldBuilder,
    Flying,
    Muted,
    Mine,
    DoorsAndSwitches,
    OpenContainers,
    AttackPlayers,
    AttackMobs,
    Operator,
    Teleportation,
    Build,
    PrivilegedBuilder,
}

impl AdventureSettingsType {
    pub fn from_ability(ability: &PlayerAbility) -> Option<Self> {
        match ability {
            PlayerAbility::Build => Some(Self::Build),
            PlayerAbility::Mine => Some(Self::Mine),
            PlayerAbility::DoorsAndSwitches => Some(Self::DoorsAndSwitches),
            PlayerAbility::OpenContainers => Some(Self::OpenContainers),
            PlayerAbility::AttackPlayers => Some(Self::AttackPlayers),
            PlayerAbility::AttackMobs => Some(Self::AttackMobs),
            PlayerAbility::OperatorCommands => Some(Self::Operator),
            PlayerAbility::Teleportation => Some(Self::Teleportation),
            PlayerAbility::Flying => Some(Self::Flying),
            PlayerAbility::Mayfly => Some(Self::AllowFlight),
            PlayerAbility::Muted => Some(Self::Muted),
            PlayerAbility::WorldBuilder => Some(Self::WorldBuilder),
            PlayerAbility::NoClip => Some(Self::NoClip),
            PlayerAbility::PrivilegedBuilder => Some(Self::PrivilegedBuilder),
            PlayerAbility::Invulnerable => Some(Self::WorldImmutable),
            _ => None,
        }
    }

    pub fn from_string(s: &str) -> Option<Self> {
        match s {
            "WORLD_IMMUTABLE" => Some(Self::WorldImmutable),
            "NO_PVM" => Some(Self::NoPVM),
            "NO_MVP" => Some(Self::NoMVP),
            "SHOW_NAMES_TAGS" => Some(Self::ShowNameTags),
            "AUTO_JUMP" => Some(Self::AutoJump),
            "ALLOW_FLIGHT" => Some(Self::AllowFlight),
            "NO_CLIP" => Some(Self::NoClip),
            "WORLD_BUILDER" => Some(Self::WorldBuilder),
            "FLYING" => Some(Self::Flying),
            "MUTED" => Some(Self::Muted),
            "MINE" => Some(Self::Mine),
            "DOORS_AND_SWITCHES" => Some(Self::DoorsAndSwitches),
            "OPEN_CONTAINERS" => Some(Self::OpenContainers),
            "ATTACK_PLAYERS" => Some(Self::AttackPlayers),
            "ATTACK_MOBS" => Some(Self::AttackMobs),
            "OPERATOR" => Some(Self::Operator),
            "TELEPORTATION" => Some(Self::Teleportation),
            "BUILD" => Some(Self::Build),
            "PrivilegedBuilder" => Some(Self::PrivilegedBuilder),
            _ => None,
        }
    }

    pub fn all() -> Vec<Self> {
        let mut abilities = Vec::new();
        abilities.push(Self::WorldImmutable);
        abilities.push(Self::NoPVM);
        abilities.push(Self::NoMVP);
        abilities.push(Self::ShowNameTags);
        abilities.push(Self::AutoJump);
        abilities.push(Self::AllowFlight);
        abilities.push(Self::NoClip);
        abilities.push(Self::WorldBuilder);
        abilities.push(Self::Flying);
        abilities.push(Self::Muted);
        abilities.push(Self::Mine);
        abilities.push(Self::DoorsAndSwitches);
        abilities.push(Self::OpenContainers);
        abilities.push(Self::AttackPlayers);
        abilities.push(Self::AttackMobs);
        abilities.push(Self::Operator);
        abilities.push(Self::Teleportation);
        abilities.push(Self::Build);
        abilities.push(Self::PrivilegedBuilder);
        abilities
    }

    pub fn is_ability(&self) -> bool {
        self.to_ability().is_some()
    }

    pub fn to_ability(&self) -> Option<PlayerAbility> {
        match self {
            Self::Mine => Some(PlayerAbility::Mine),
            Self::DoorsAndSwitches => Some(PlayerAbility::DoorsAndSwitches),
            Self::OpenContainers => Some(PlayerAbility::OpenContainers),
            Self::AttackPlayers => Some(PlayerAbility::AttackPlayers),
            Self::AttackMobs => Some(PlayerAbility::AttackMobs),
            Self::Operator => Some(PlayerAbility::OperatorCommands),
            Self::Teleportation => Some(PlayerAbility::Teleportation),
            Self::Flying => Some(PlayerAbility::Flying),
            Self::AllowFlight => Some(PlayerAbility::Mayfly),
            Self::Muted => Some(PlayerAbility::Muted),
            Self::WorldBuilder => Some(PlayerAbility::WorldBuilder),
            Self::NoClip => Some(PlayerAbility::NoClip),
            Self::PrivilegedBuilder => Some(PlayerAbility::PrivilegedBuilder),
            Self::WorldImmutable => Some(PlayerAbility::Invulnerable),
            _ => None,
        }
    }
}

#[derive(Component)]
pub struct AdventureSettings {
    entity_id: EntityId,
    world: World,
    map: Arc<RwLock<HashMap<AdventureSettingsType, bool>>>,
    command_permission: Arc<RwLock<CommandPermission>>,
    player_permission: Arc<RwLock<PlayerPermission>>,
}

impl AdventureSettings {
    pub fn new(entity_id: EntityId, world: World) -> Option<Self> {
        Self::init(
            Self {
                entity_id,
                world,
                map: Arc::new(RwLock::new(HashMap::new())),
                command_permission: Arc::new(RwLock::new(CommandPermission::Normal)),
                player_permission: Arc::new(RwLock::new(PlayerPermission::Visitor)),
            },
            None,
        )
    }

    pub fn new_with_nbt(entity_id: EntityId, world: World, nbt: CompoundNbt) -> Option<Self> {
        Self::init(
            Self {
                entity_id,
                world,
                map: Arc::new(RwLock::new(HashMap::new())),
                command_permission: Arc::new(RwLock::new(CommandPermission::Normal)),
                player_permission: Arc::new(RwLock::new(PlayerPermission::Visitor)),
            },
            Some(nbt),
        )
    }

    fn init(self, nbt: Option<CompoundNbt>) -> Option<Self> {
        let data = {
            let data = self
                .world
                .get_component::<MinecraftClient>(&self.entity_id)
                .unwrap();
            let data = (*data.data.read()).clone();
            data
        };
        if let Some(nbt) = nbt {
            if nbt.contains_key("Abilities") {
                self.read_nbt(nbt)?
            } else {
                self.init0(data.clone())
            }
        } else {
            self.init0(data.clone())
        }

        if *self.player_permission.read() == PlayerPermission::Operator && !data.is_op {
            self.update_op(false);
        }
        if *self.player_permission.read() != PlayerPermission::Operator && data.is_op {
            self.update_op(true);
        }
        Some(self)
    }

    /// `AdventureSettings.Type` defaults (fallback when NBT keys are missing):
    /// MINE / DOORS_AND_SWITCHES / OPEN_CONTAINERS / ATTACK_PLAYERS /
    /// ATTACK_MOBS / BUILD default to true, the rest to false. Without these,
    /// the emitted ability lacks container/build/mine bits (the hard gate for opening workbenches).
    fn apply_defaults(&self) {
        use AdventureSettingsType as T;
        self.set(T::WorldImmutable, false)
            .set(T::NoPVM, false)
            .set(T::NoMVP, false)
            .set(T::ShowNameTags, false)
            .set(T::AutoJump, true)
            .set(T::AllowFlight, false)
            .set(T::NoClip, false)
            .set(T::WorldBuilder, false)
            .set(T::Flying, false)
            .set(T::Muted, false)
            .set(T::Mine, true)
            .set(T::DoorsAndSwitches, true)
            .set(T::OpenContainers, true)
            .set(T::AttackPlayers, true)
            .set(T::AttackMobs, true)
            .set(T::Operator, false)
            .set(T::Teleportation, false)
            .set(T::Build, true)
            .set(T::PrivilegedBuilder, false);
    }

    fn init0(&self, data: MinecraftClientData) {
        let gamemode = data.gamemode;
        self.apply_defaults();
        self.set(
            AdventureSettingsType::WorldImmutable,
            gamemode.is_adventure() || gamemode.is_spectator(),
        )
        .set(
            AdventureSettingsType::WorldBuilder,
            !gamemode.is_adventure() && !gamemode.is_spectator(),
        )
        .set(AdventureSettingsType::AutoJump, true)
        .set(
            AdventureSettingsType::AllowFlight,
            gamemode.is_creative() || gamemode.is_spectator(),
        )
        .set(AdventureSettingsType::NoClip, gamemode.is_spectator())
        .set(AdventureSettingsType::Flying, gamemode.is_spectator())
        .set(AdventureSettingsType::Operator, data.is_op)
        .set(AdventureSettingsType::Teleportation, data.is_op);
        *self.command_permission.write() = if data.is_op {
            CommandPermission::Operator
        } else {
            CommandPermission::Normal
        };
        *self.player_permission.write() = if data.is_op {
            PlayerPermission::Operator
        } else {
            PlayerPermission::Member
        };
    }

    pub fn set_ability(&self, ability: &PlayerAbility, value: bool) -> Option<&Self> {
        Some(self.set(AdventureSettingsType::from_ability(ability)?, value))
    }

    pub fn set(&self, setting: AdventureSettingsType, value: bool) -> &Self {
        self.map.write().insert(setting, value);
        self
    }

    pub fn get(&self, setting: &AdventureSettingsType) -> bool {
        self.map.read().get(setting).unwrap_or(&false).clone()
    }

    fn read_nbt(&self, nbt: CompoundNbt) -> Option<()> {
        // Lay defaults first, then overlay NBT, so old saves missing keys keep their abilities.
        self.apply_defaults();
        let abilities = nbt.get("Abilities")?.as_compound()?;
        for (name, value) in abilities.iter() {
            let Some(setting) = AdventureSettingsType::from_string(name) else {
                continue;
            };
            if let Some(value) = value.as_i32() {
                self.set(setting, value == 1);
            }
        }
        *self.player_permission.write() =
            PlayerPermission::from_string(nbt.get("PlayerPermission")?.as_string()?)?;
        *self.command_permission.write() =
            CommandPermission::from_string(nbt.get("CommandPermission")?.as_string()?)?;
        Some(())
    }

    pub fn update_op(&self, op: bool) {
        if op {
            for ability in PlayerAbility::CONTROLLABLE_ABILITIES {
                self.set_ability(ability, true);
            }
        }
        self.set(AdventureSettingsType::Operator, op)
            .set(AdventureSettingsType::Teleportation, op);
        *self.command_permission.write() = if op {
            CommandPermission::Operator
        } else {
            CommandPermission::Normal
        };
        if op && *self.player_permission.read() != PlayerPermission::Operator {
            *self.player_permission.write() = PlayerPermission::Operator;
        }

        if !op && *self.player_permission.read() == PlayerPermission::Operator {
            *self.player_permission.write() = PlayerPermission::Member;
        }
    }

    pub fn entity_id(&self) -> MinecraftEntityId {
        *self
            .world
            .get_component::<MinecraftEntityId>(&self.entity_id)
            .unwrap()
    }

    pub fn world(&self) -> (World, EntityId) {
        (self.world.clone(), self.entity_id)
    }

    pub fn command_permission(&self) -> CommandPermission {
        *self.command_permission.read()
    }

    pub fn player_permission(&self) -> PlayerPermission {
        *self.player_permission.read()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn survival_settings() -> AdventureSettings {
        let world = World::new();
        let entity =
            world.spawn(sc_utils::components::DisplayName("tester".to_string()));
        let client = MinecraftClient::new(
            entity,
            world.clone(),
            sc_utils::world::client_data::MinecraftClientData::default(),
        );
        world.add_component(&entity, client);
        AdventureSettings::new(entity, world).expect("settings init")
    }

    #[test]
    fn survival_defaults_match_pnx_container_requirements() {
        // Type defaults: container/mine/build/attack all true; flying/noclip/OP all false.
        // Without OPEN_CONTAINERS, the client closes workbench windows immediately.
        let settings = survival_settings();
        for ty in [
            AdventureSettingsType::Mine,
            AdventureSettingsType::DoorsAndSwitches,
            AdventureSettingsType::OpenContainers,
            AdventureSettingsType::AttackPlayers,
            AdventureSettingsType::AttackMobs,
            AdventureSettingsType::Build,
            AdventureSettingsType::WorldBuilder,
            AdventureSettingsType::AutoJump,
        ] {
            assert!(settings.get(&ty), "{ty:?} must default true");
        }
        for ty in [
            AdventureSettingsType::WorldImmutable,
            AdventureSettingsType::AllowFlight,
            AdventureSettingsType::NoClip,
            AdventureSettingsType::Flying,
            AdventureSettingsType::Operator,
            AdventureSettingsType::Teleportation,
            AdventureSettingsType::Muted,
            AdventureSettingsType::PrivilegedBuilder,
        ] {
            assert!(!settings.get(&ty), "{ty:?} must default false");
        }
    }
}
