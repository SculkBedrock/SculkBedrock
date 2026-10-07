use lazy_static::lazy_static;
use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{BinaryIo, ByteReader, ByteWriter};
use sc_entity::MinecraftEntityId;
use sc_network_macros::MinecraftPacket;
use sc_utils::game::structs::permission::{CommandPermission, PlayerPermission};
use sc_utils::game::structs::player_ability::{PlayerAbility, PlayerAbilityLayer};
use std::collections::{HashMap, HashSet};
use std::io::{Error, ErrorKind};

#[derive(Clone, Debug, MinecraftPacket)]
pub struct Disconnect {
    pub hide_disconnect_screen: bool,
    pub kick_message: String,
}

impl Writer for Disconnect {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        // failReason (varint) + hideDisconnectionScreen (bool),
        // followed by message + filteredMessage unless hidden.
        buf.write_var_i32(0)?; // Disconnect fail reason = UNKNOWN
        buf.write_bool(self.hide_disconnect_screen)?;
        if !self.hide_disconnect_screen {
            buf.write_string(&self.kick_message)?;
            buf.write_string(&self.kick_message)?; // filteredMessage mirrors message
        }
        Ok(())
    }
}

impl Reader<Disconnect> for Disconnect {
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        let _reason = buf.read_var_i32()?;
        let hide_disconnect_screen = buf.read_bool()?;
        let (kick_message, _filtered_message) = if hide_disconnect_screen {
            (String::new(), String::new())
        } else {
            (buf.read_string()?, buf.read_string()?)
        };
        Ok(Self {
            hide_disconnect_screen,
            kick_message,
        })
    }
}

lazy_static! {
    pub static ref FLAGS_TO_BITS: HashMap<PlayerAbility, i32> = UpdateAbilities::flags_to_bits();
}

#[derive(Clone, Debug, MinecraftPacket)]
pub struct UpdateAbilities {
    pub entity_id: MinecraftEntityId,
    pub player_permission: PlayerPermission,
    pub command_permission: CommandPermission,
    pub ability_layers: Vec<PlayerAbilityLayer>,
}

impl UpdateAbilities {
    pub const VALID_FLAGS: &'static [PlayerAbility] = &[
        PlayerAbility::Build,
        PlayerAbility::Mine,
        PlayerAbility::DoorsAndSwitches,
        PlayerAbility::OpenContainers,
        PlayerAbility::AttackPlayers,
        PlayerAbility::AttackMobs,
        PlayerAbility::OperatorCommands,
        PlayerAbility::Teleportation,
        PlayerAbility::Invulnerable,
        PlayerAbility::Flying,
        PlayerAbility::Mayfly,
        PlayerAbility::Instabuild,
        PlayerAbility::Lightning,
        PlayerAbility::FlySpeed,
        PlayerAbility::WalkSpeed,
        PlayerAbility::Muted,
        PlayerAbility::WorldBuilder,
        PlayerAbility::NoClip,
        PlayerAbility::PrivilegedBuilder,
        PlayerAbility::VerticalFlySpeed,
    ];

    fn flags_to_bits() -> HashMap<PlayerAbility, i32> {
        let mut map = HashMap::new();
        for i in 0..Self::VALID_FLAGS.len() {
            map.insert(Self::VALID_FLAGS[i].clone(), 1 << i);
        }
        map
    }
    fn write_ability_layer(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        for layer in &self.ability_layers {
            buf.write_i16_le(layer.layer_type.index() as i16)?;
            buf.write_i32_le(Self::get_abilities_number(&layer.ability_set))?;
            buf.write_i32_le(Self::get_abilities_number(&layer.ability_value))?;
            buf.write_f32_le(layer.fly_speed)?;
            buf.write_f32_le(layer.vertical_fly_speed)?;
            buf.write_f32_le(layer.walk_speed)?;
        }
        Ok(())
    }

    fn get_abilities_number(abilities: &HashSet<PlayerAbility>) -> i32 {
        let mut abilities_number = 0;
        for ability in abilities {
            abilities_number |= *(FLAGS_TO_BITS.get(ability).unwrap_or(&0));
        }
        abilities_number
    }
}

impl Writer for UpdateAbilities {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_i64_le(self.entity_id.0 as i64)?;
        buf.write_var_u32(self.player_permission.index() as u32)?;
        buf.write_var_u32(self.command_permission.index() as u32)?;
        buf.write_var_u32(self.ability_layers.len() as u32)?;
        self.write_ability_layer(buf)
    }
}

impl Reader<UpdateAbilities> for UpdateAbilities {
    fn read(_buf: &mut ByteReader) -> Result<UpdateAbilities, Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "UpdateAbilities decode is unsupported",
        ))
    }
}

#[derive(Clone, Debug, BinaryIo, MinecraftPacket)]
pub struct UpdateAdventureSettings {
    pub no_pvm: bool,
    pub no_mvp: bool,
    pub immutable_world: bool,
    pub show_name_tags: bool,
    pub auto_jump: bool,
}

/// SetPlayerGameType (0x3e), server -> client.
///
/// Game type ordinal matches `Gamemode::to_i32`:
/// Survival 0 / Creative 1 / Adventure 2 / Spectator 6.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct SetPlayerGameType {
    pub gamemode: i32,
}

impl Writer for SetPlayerGameType {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_i32(self.gamemode)
    }
}

impl Reader<SetPlayerGameType> for SetPlayerGameType {
    fn read(_buf: &mut ByteReader) -> Result<Self, Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "SetPlayerGameType decode is unsupported",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{SetPlayerGameType, UpdateAbilities};
    use sc_binary::interfaces::Writer;
    use sc_binary::ByteWriter;
    use sc_entity::MinecraftEntityId;
    use sc_utils::game::structs::permission::{CommandPermission, PlayerPermission};
    use sc_utils::game::structs::player_ability::{
        AbilityLayerType, PlayerAbility, PlayerAbilityLayer,
    };
    use std::collections::HashSet;

    #[test]
    fn update_abilities_matches_layer_layout() {
        let mut layer = PlayerAbilityLayer::new(AbilityLayerType::Base);
        layer.ability_set = HashSet::from([PlayerAbility::Build, PlayerAbility::Mine]);
        layer.ability_value = HashSet::from([PlayerAbility::Mayfly]);

        let packet = UpdateAbilities {
            entity_id: MinecraftEntityId(7),
            player_permission: PlayerPermission::Member,
            command_permission: CommandPermission::Normal,
            ability_layers: vec![layer],
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();

        assert_eq!(
            writer.as_slice(),
            &[
                7, 0, 0, 0, 0, 0, 0, 0, // entity id, LLong
                1, 0, 1, // permissions and layer array count
                1, 0, // Base layer, LShort
                3, 0, 0, 0, // abilities set: Build | Mine
                0, 4, 0, 0, // ability values: Mayfly
                0xcd, 0xcc, 0x4c, 0x3d, // fly speed
                0, 0, 0x80, 0x3f, // vertical fly speed
                0xcd, 0xcc, 0xcc, 0x3d, // walk speed
            ]
        );
    }

    #[test]
    fn set_player_game_type_matches_varint_ordinal() {
        // GameType ordinal = Gamemode::to_i32: 0/1/2/6 as zigzag varint.
        for (gamemode, expected) in [
            (0, vec![0x00]),
            (1, vec![0x02]),
            (2, vec![0x04]),
            (6, vec![0x0c]),
        ] {
            let mut writer = ByteWriter::new();
            SetPlayerGameType { gamemode }.write(&mut writer).unwrap();
            assert_eq!(writer.as_slice(), &expected);
        }
    }
}
