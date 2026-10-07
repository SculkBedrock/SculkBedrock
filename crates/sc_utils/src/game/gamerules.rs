use serde::{Deserialize, Deserializer};
use std::collections::HashMap;

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub enum GameRule {
    CommandBlocksEnabled,
    CommandBlockOutput,
    DoDaylightCycle,
    DoEntityDrops,
    DoFireTick,
    DoInsomnia,
    DoImmediateRespawn,
    DoMobLoot,
    DoMobSpawning,
    DoTileDrops,
    DoWeatherCycle,
    DrowningDamage,
    FallDamage,
    FireDamage,
    FreezeDamage,
    FunctionCommandLimit,
    KeepInventory,
    MaxCommandChainLength,
    MobGriefing,
    NaturalRegeneration,
    PVP,
    RandomTickSpeed,
    SendCommandFeedback,
    ShowCoordinates,
    ShowDeathMessages,
    SpawnRadius,
    TntExplodes,
    ExperimentalGameplay,
    ShowTags,
    PlayersSleepingPercentage,
    DoLimitedCrafting,
    RespawnBlocksExplode,
    ShowBorderEffect,
    RecipesUnlock,
    ShowDaysPlayed,
}

impl GameRule {
    pub fn name(&self) -> &str {
        match self {
            GameRule::CommandBlocksEnabled => "commandBlocksEnabled",
            GameRule::CommandBlockOutput => "commandBlockOutput",
            GameRule::DoDaylightCycle => "doDaylightCycle",
            GameRule::DoEntityDrops => "doEntityDrops",
            GameRule::DoFireTick => "doFireTick",
            GameRule::DoInsomnia => "doInsomnia",
            GameRule::DoImmediateRespawn => "doImmediateRespawn",
            GameRule::DoMobLoot => "doMobLoot",
            GameRule::DoMobSpawning => "doMobSpawning",
            GameRule::DoTileDrops => "doTileDrops",
            GameRule::DoWeatherCycle => "doWeatherCycle",
            GameRule::DrowningDamage => "drowningDamage",
            GameRule::FallDamage => "fallDamage",
            GameRule::FireDamage => "fireDamage",
            GameRule::FreezeDamage => "freezeDamage",
            GameRule::FunctionCommandLimit => "functionCommandLimit",
            GameRule::KeepInventory => "keepInventory",
            GameRule::MaxCommandChainLength => "maxCommandChainLength",
            GameRule::MobGriefing => "mobGriefing",
            GameRule::NaturalRegeneration => "naturalRegeneration",
            GameRule::PVP => "pvp",
            GameRule::RandomTickSpeed => "randomTickSpeed",
            GameRule::SendCommandFeedback => "sendCommandFeedback",
            GameRule::ShowCoordinates => "showCoordinates",
            GameRule::ShowDeathMessages => "showDeathMessages",
            GameRule::SpawnRadius => "spawnRadius",
            GameRule::TntExplodes => "tntExplodes",
            GameRule::ExperimentalGameplay => "experimentalGameplay",
            GameRule::ShowTags => "showTags",
            GameRule::PlayersSleepingPercentage => "playersSleepingPercentage",
            GameRule::DoLimitedCrafting => "doLimitedCrafting",
            GameRule::RespawnBlocksExplode => "respawnBlocksExplode",
            GameRule::ShowBorderEffect => "showBorderEffect",
            GameRule::RecipesUnlock => "recipesUnlock",
            GameRule::ShowDaysPlayed => "showDaysPlayed",
        }
    }

    /// Returns all known GameRule variants for iteration.
    pub fn values() -> &'static [GameRule] {
        &[
            GameRule::CommandBlocksEnabled,
            GameRule::CommandBlockOutput,
            GameRule::DoDaylightCycle,
            GameRule::DoEntityDrops,
            GameRule::DoFireTick,
            GameRule::DoInsomnia,
            GameRule::DoImmediateRespawn,
            GameRule::DoMobLoot,
            GameRule::DoMobSpawning,
            GameRule::DoTileDrops,
            GameRule::DoWeatherCycle,
            GameRule::DrowningDamage,
            GameRule::FallDamage,
            GameRule::FireDamage,
            GameRule::FreezeDamage,
            GameRule::FunctionCommandLimit,
            GameRule::KeepInventory,
            GameRule::MaxCommandChainLength,
            GameRule::MobGriefing,
            GameRule::NaturalRegeneration,
            GameRule::PVP,
            GameRule::RandomTickSpeed,
            GameRule::SendCommandFeedback,
            GameRule::ShowCoordinates,
            GameRule::ShowDeathMessages,
            GameRule::SpawnRadius,
            GameRule::TntExplodes,
            GameRule::ShowTags,
            GameRule::PlayersSleepingPercentage,
            GameRule::DoLimitedCrafting,
            GameRule::RespawnBlocksExplode,
            GameRule::ShowBorderEffect,
            GameRule::RecipesUnlock,
            GameRule::ShowDaysPlayed,
        ]
    }
}

#[derive(Clone, Debug)]
pub enum GameRuleType {
    Unknown,
    Bool(bool),
    Int(i32),
    Float(f32),
}

impl GameRuleType {
    pub fn index(&self) -> usize {
        match self {
            GameRuleType::Unknown => 0,
            GameRuleType::Bool(_) => 1,
            GameRuleType::Int(_) => 2,
            GameRuleType::Float(_) => 3,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GameRuleValue {
    pub can_be_changed: bool,
    pub value: GameRuleType,
}

impl GameRuleValue {
    pub fn new(value: GameRuleType) -> Self {
        Self {
            can_be_changed: false,
            value,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GameRules {
    map: HashMap<GameRule, GameRuleValue>,
}

impl GameRules {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn change(&mut self, rule: GameRule, value: GameRuleValue) {
        self.map.insert(rule, value);
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&GameRule, &GameRuleValue)> {
        self.map.iter()
    }
}

impl Default for GameRules {
    fn default() -> Self {
        let mut map = HashMap::new();
        map.insert(
            GameRule::CommandBlocksEnabled,
            GameRuleValue::new(GameRuleType::Bool(true)),
        );
        map.insert(
            GameRule::CommandBlockOutput,
            GameRuleValue::new(GameRuleType::Bool(true)),
        );
        map.insert(
            GameRule::DoDaylightCycle,
            GameRuleValue::new(GameRuleType::Bool(true)),
        );
        map.insert(
            GameRule::DoEntityDrops,
            GameRuleValue::new(GameRuleType::Bool(true)),
        );
        map.insert(
            GameRule::DoFireTick,
            GameRuleValue::new(GameRuleType::Bool(true)),
        );
        map.insert(
            GameRule::DoInsomnia,
            GameRuleValue::new(GameRuleType::Bool(true)),
        );
        map.insert(
            GameRule::DoImmediateRespawn,
            GameRuleValue::new(GameRuleType::Bool(true)),
        );
        map.insert(
            GameRule::DoMobLoot,
            GameRuleValue::new(GameRuleType::Bool(true)),
        );
        map.insert(
            GameRule::DoMobSpawning,
            GameRuleValue::new(GameRuleType::Bool(true)),
        );
        map.insert(
            GameRule::DoTileDrops,
            GameRuleValue::new(GameRuleType::Bool(true)),
        );
        map.insert(
            GameRule::DoWeatherCycle,
            GameRuleValue::new(GameRuleType::Bool(true)),
        );
        map.insert(
            GameRule::DrowningDamage,
            GameRuleValue::new(GameRuleType::Bool(true)),
        );
        map.insert(
            GameRule::FallDamage,
            GameRuleValue::new(GameRuleType::Bool(true)),
        );
        map.insert(
            GameRule::FireDamage,
            GameRuleValue::new(GameRuleType::Bool(true)),
        );
        map.insert(
            GameRule::FreezeDamage,
            GameRuleValue::new(GameRuleType::Bool(true)),
        );
        map.insert(
            GameRule::FunctionCommandLimit,
            GameRuleValue::new(GameRuleType::Int(10000)),
        );
        map.insert(
            GameRule::KeepInventory,
            GameRuleValue::new(GameRuleType::Bool(false)),
        );
        map.insert(
            GameRule::MaxCommandChainLength,
            GameRuleValue::new(GameRuleType::Int(65536)),
        );
        map.insert(
            GameRule::MobGriefing,
            GameRuleValue::new(GameRuleType::Bool(true)),
        );
        map.insert(
            GameRule::NaturalRegeneration,
            GameRuleValue::new(GameRuleType::Bool(true)),
        );
        map.insert(GameRule::PVP, GameRuleValue::new(GameRuleType::Bool(true)));
        map.insert(
            GameRule::RandomTickSpeed,
            GameRuleValue::new(GameRuleType::Int(3)),
        );
        map.insert(
            GameRule::SendCommandFeedback,
            GameRuleValue::new(GameRuleType::Bool(true)),
        );
        map.insert(
            GameRule::ShowCoordinates,
            GameRuleValue::new(GameRuleType::Bool(false)),
        );
        map.insert(
            GameRule::ShowDeathMessages,
            GameRuleValue::new(GameRuleType::Bool(true)),
        );
        map.insert(
            GameRule::SpawnRadius,
            GameRuleValue::new(GameRuleType::Int(5)),
        );
        map.insert(
            GameRule::TntExplodes,
            GameRuleValue::new(GameRuleType::Bool(true)),
        );
        map.insert(
            GameRule::ShowTags,
            GameRuleValue::new(GameRuleType::Bool(true)),
        );
        map.insert(
            GameRule::ExperimentalGameplay,
            GameRuleValue::new(GameRuleType::Bool(true)),
        );
        map.insert(
            GameRule::PlayersSleepingPercentage,
            GameRuleValue::new(GameRuleType::Int(100)),
        );
        map.insert(
            GameRule::DoLimitedCrafting,
            GameRuleValue::new(GameRuleType::Bool(false)),
        );
        map.insert(
            GameRule::RespawnBlocksExplode,
            GameRuleValue::new(GameRuleType::Bool(true)),
        );
        map.insert(
            GameRule::ShowBorderEffect,
            GameRuleValue::new(GameRuleType::Bool(true)),
        );
        map.insert(
            GameRule::ShowDaysPlayed,
            GameRuleValue::new(GameRuleType::Bool(false)),
        );
        map.insert(
            GameRule::RecipesUnlock,
            GameRuleValue::new(GameRuleType::Bool(false)),
        );
        Self { map }
    }
}

impl<'de> Deserialize<'de> for GameRules {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Self::new())
    }
}
