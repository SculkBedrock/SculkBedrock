use crate::player_connection::PlayerConnection;
use crate::protocol::server::game::{UpdateAbilities, UpdateAdventureSettings};
use crate::utils::server::broadcast_packet_from_world;
use async_trait::async_trait;
use sc_ecs::entity::EntityId;
use sc_entity::player::adventure_settings::{AdventureSettings, AdventureSettingsType};
use sc_log::t_log;
use sc_packloader::definitions::attribute::EntityAttributes;
use sc_utils::game::client::MinecraftClient;
use sc_utils::game::structs::player_ability::{
    AbilityLayerType, PlayerAbility, PlayerAbilityLayer,
};
use sc_utils::game::structs::server::Server;

#[async_trait]
pub trait SCNetworkAdventureSettings {
    async fn send_abilities(&self, players: Vec<EntityId>);
    async fn update(&self);
}

#[async_trait]
impl SCNetworkAdventureSettings for AdventureSettings {
    async fn send_abilities(&self, players: Vec<EntityId>) {
        let entity_id = self.entity_id();
        let (world, ability_player) = self.world();
        let mut layer = PlayerAbilityLayer::new(AbilityLayerType::Base);
        layer.ability_set.extend(PlayerAbility::all());
        for ty in AdventureSettingsType::all() {
            if ty.is_ability() && self.get(&ty) {
                if let Some(ability) = ty.to_ability() {
                    layer.ability_value.insert(ability);
                }
            }
        }

        let Some(gamemode) = world
            .get_component::<MinecraftClient>(&ability_player)
            .map(|client| client.data.read().gamemode)
        else {
            log::warn!(
                "{}",
                t_log!(
                    "console.player.abilities_missing",
                    action = "send abilities",
                    missing = "MinecraftClient"
                )
            );
            return;
        };
        if gamemode.is_creative() {
            layer.ability_value.insert(PlayerAbility::Instabuild);
        }

        layer.ability_value.insert(PlayerAbility::WalkSpeed);
        layer.ability_value.insert(PlayerAbility::FlySpeed);

        let Some(attributes) = world
            .get_component::<EntityAttributes>(&ability_player)
            .map(|attributes| (*attributes).clone())
        else {
            log::warn!(
                "{}",
                t_log!(
                    "console.player.abilities_missing",
                    action = "send abilities",
                    missing = "EntityAttributes"
                )
            );
            return;
        };
        let Some(movement_speed) = attributes.get(EntityAttributes::MOVEMENT_SPEED) else {
            log::warn!(
                "{}",
                t_log!(
                    "console.player.abilities_missing",
                    action = "send abilities",
                    missing = "movement speed attribute"
                )
            );
            return;
        };
        let Some(flight_speed) = attributes.get(EntityAttributes::FLIGHT_SPEED) else {
            log::warn!(
                "{}",
                t_log!(
                    "console.player.abilities_missing",
                    action = "send abilities",
                    missing = "flight speed attribute"
                )
            );
            return;
        };
        layer.walk_speed = movement_speed.default_value;
        layer.fly_speed = flight_speed.default_value;

        let packet = UpdateAbilities {
            entity_id,
            player_permission: self.player_permission(),
            command_permission: self.command_permission(),
            ability_layers: vec![layer],
        };
        let Some(server_world) = Server::global().map(|server| server.world.clone()) else {
            return;
        };
        broadcast_packet_from_world(server_world, players, packet, false).await;
    }

    async fn update(&self) {
        let (world, player) = self.world();
        let Some(connection) = world
            .get_component::<PlayerConnection>(&player)
            .map(|connection| connection.clone())
        else {
            log::warn!(
                "{}",
                t_log!(
                    "console.player.abilities_missing",
                    action = "update adventure settings",
                    missing = "PlayerConnection"
                )
            );
            return;
        };
        let Some(client) = world
            .get_component::<MinecraftClient>(&player)
            .map(|client| client.clone())
        else {
            log::warn!(
                "{}",
                t_log!(
                    "console.player.abilities_missing",
                    action = "update adventure settings",
                    missing = "MinecraftClient"
                )
            );
            return;
        };

        // 1. Send UpdateAbilities first.
        let entity_id = self.entity_id();
        let mut layer = PlayerAbilityLayer::new(AbilityLayerType::Base);
        layer.ability_set.extend(PlayerAbility::all());
        for ty in AdventureSettingsType::all() {
            if ty.is_ability() && self.get(&ty) {
                if let Some(ability) = ty.to_ability() {
                    layer.ability_value.insert(ability);
                }
            }
        }

        let gamemode = client.data.read().gamemode;
        if gamemode.is_creative() {
            layer.ability_value.insert(PlayerAbility::Instabuild);
        }
        layer.ability_value.insert(PlayerAbility::WalkSpeed);
        layer.ability_value.insert(PlayerAbility::FlySpeed);

        let Some(attributes) = world
            .get_component::<EntityAttributes>(&player)
            .map(|attributes| (*attributes).clone())
        else {
            log::warn!(
                "{}",
                t_log!(
                    "console.player.abilities_missing",
                    action = "update adventure settings",
                    missing = "EntityAttributes"
                )
            );
            return;
        };
        let Some(movement_speed) = attributes.get(EntityAttributes::MOVEMENT_SPEED) else {
            log::warn!(
                "{}",
                t_log!(
                    "console.player.abilities_missing",
                    action = "update adventure settings",
                    missing = "movement speed attribute"
                )
            );
            return;
        };
        let Some(flight_speed) = attributes.get(EntityAttributes::FLIGHT_SPEED) else {
            log::warn!(
                "{}",
                t_log!(
                    "console.player.abilities_missing",
                    action = "update adventure settings",
                    missing = "flight speed attribute"
                )
            );
            return;
        };
        layer.walk_speed = movement_speed.default_value;
        layer.fly_speed = flight_speed.default_value;

        let abilities_packet = UpdateAbilities {
            entity_id,
            player_permission: self.player_permission(),
            command_permission: self.command_permission(),
            ability_layers: vec![layer],
        };
        let _ = connection.send_packet(abilities_packet, true).await;

        // 2. Send UpdateAdventureSettings second.
        let _ = connection
            .send_packet(
                UpdateAdventureSettings {
                    no_pvm: self.get(&AdventureSettingsType::NoPVM),
                    no_mvp: self.get(&AdventureSettingsType::NoMVP),
                    immutable_world: self.get(&AdventureSettingsType::WorldImmutable),
                    show_name_tags: self.get(&AdventureSettingsType::ShowNameTags),
                    auto_jump: self.get(&AdventureSettingsType::AutoJump),
                },
                true,
            )
            .await;
        client.reset_in_air_ticks();
    }
}
