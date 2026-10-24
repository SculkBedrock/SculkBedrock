use lazy_static::lazy_static;
use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{BinaryIo, ByteReader, ByteWriter};
use sc_entity::MinecraftEntityId;
use sc_network_macros::MinecraftPacket;
use sc_utils::game::structs::permission::{CommandPermission, PlayerPermission};
use sc_utils::game::structs::player_ability::{PlayerAbility, PlayerAbilityLayer};
use std::collections::{HashMap, HashSet};
use std::io::{Error, ErrorKind};

/// DisconnectPacket：踢出/断开连接，可携带原因消息。
///
/// DisconnectPacket: kicks or disconnects a connection, optionally with a
/// reason message.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct Disconnect {
    /// 是否隐藏断开屏幕（隐藏时不发送消息）。
    ///
    /// Whether to hide the disconnect screen (no message is sent when
    /// hidden).
    pub hide_disconnect_screen: bool,
    /// 踢出原因消息。
    ///
    /// Kick reason message.
    pub kick_message: String,
}

impl Writer for Disconnect {
    /// 写入 Disconnect 包体。
    ///
    /// Writes the Disconnect packet body.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
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
    /// 解码 Disconnect 包体。
    ///
    /// Decodes a Disconnect packet body.
    ///
    /// - `buf`: 输入字节读取器
    ///
    /// # Errors
    /// 缓冲区不足时返回解码错误。
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
    /// 能力标志到位掩码的全局映射表（由 [`UpdateAbilities::flags_to_bits`] 构建）。
    ///
    /// Global ability-flag to bit-mask map (built by
    /// [`UpdateAbilities::flags_to_bits`]).
    pub static ref FLAGS_TO_BITS: HashMap<PlayerAbility, i32> = UpdateAbilities::flags_to_bits();
}

/// UpdateAbilitiesPacket：同步玩家能力（飞行、建造、无敌等）。
///
/// UpdateAbilitiesPacket: synchronizes player abilities (fly, build,
/// invulnerability, etc.).
#[derive(Clone, Debug, MinecraftPacket)]
pub struct UpdateAbilities {
    /// 目标实体 id。
    ///
    /// Target entity id.
    pub entity_id: MinecraftEntityId,
    /// 玩家权限等级。
    ///
    /// Player permission level.
    pub player_permission: PlayerPermission,
    /// 命令权限等级。
    ///
    /// Command permission level.
    pub command_permission: CommandPermission,
    /// 能力层列表。
    ///
    /// Ability layers.
    pub ability_layers: Vec<PlayerAbilityLayer>,
}

impl UpdateAbilities {
    /// 可编码为位标志的合法能力集合（顺序即位序）。
    ///
    /// The set of abilities encodable as bit flags (order defines bit
    /// positions).
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

    /// 构建能力到位掩码的映射表。
    ///
    /// Builds the ability-to-bit-mask map.
    ///
    /// # Returns
    /// 每个能力对应 `1 << 位序` 的 HashMap。
    fn flags_to_bits() -> HashMap<PlayerAbility, i32> {
        let mut map = HashMap::new();
        for i in 0..Self::VALID_FLAGS.len() {
            map.insert(Self::VALID_FLAGS[i].clone(), 1 << i);
        }
        map
    }
    /// 写入全部能力层。
    ///
    /// Writes all ability layers.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
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

    /// 将能力集合编码为 32 位掩码。
    ///
    /// Encodes an ability set into a 32-bit mask.
    ///
    /// - `abilities`: 能力集合
    ///
    /// # Returns
    /// 置位的位掩码；未知能力按 0 处理。
    fn get_abilities_number(abilities: &HashSet<PlayerAbility>) -> i32 {
        let mut abilities_number = 0;
        for ability in abilities {
            abilities_number |= *(FLAGS_TO_BITS.get(ability).unwrap_or(&0));
        }
        abilities_number
    }
}

impl Writer for UpdateAbilities {
    /// 写入 UpdateAbilities 包体。
    ///
    /// Writes the UpdateAbilities packet body.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_i64_le(self.entity_id.0 as i64)?;
        buf.write_var_u32(self.player_permission.index() as u32)?;
        buf.write_var_u32(self.command_permission.index() as u32)?;
        buf.write_var_u32(self.ability_layers.len() as u32)?;
        self.write_ability_layer(buf)
    }
}

impl Reader<UpdateAbilities> for UpdateAbilities {
    /// 不支持解码：服务端只发送能力更新。
    ///
    /// Decoding is unsupported: the server only sends ability updates.
    ///
    /// # Errors
    /// 始终返回 [`std::io::ErrorKind::Unsupported`] 错误。
    fn read(_buf: &mut ByteReader) -> Result<UpdateAbilities, Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "UpdateAbilities decode is unsupported",
        ))
    }
}

/// UpdateAdventureSettingsPacket：同步冒险设置开关。
///
/// UpdateAdventureSettingsPacket: synchronizes adventure setting flags.
#[derive(Clone, Debug, BinaryIo, MinecraftPacket)]
pub struct UpdateAdventureSettings {
    /// 是否禁止玩家间伤害（PvP）。
    ///
    /// Whether player-vs-player damage is disabled.
    pub no_pvm: bool,
    /// 是否禁止生物伤害玩家。
    ///
    /// Whether mob-vs-player damage is disabled.
    pub no_mvp: bool,
    /// 世界是否不可变。
    ///
    /// Whether the world is immutable.
    pub immutable_world: bool,
    /// 是否显示名牌。
    ///
    /// Whether name tags are shown.
    pub show_name_tags: bool,
    /// 是否开启自动跳跃。
    ///
    /// Whether auto-jump is enabled.
    pub auto_jump: bool,
}

/// SetPlayerGameTypePacket (0x3e)，服务端 → 客户端：设置玩家游戏模式。
///
/// SetPlayerGameTypePacket (0x3e), server -> client: sets the player game
/// mode.
///
/// 游戏模式序数与 `Gamemode::to_i32` 一致：
/// 生存 0 / 创造 1 / 冒险 2 / 旁观 6。
#[derive(Clone, Debug, MinecraftPacket)]
pub struct SetPlayerGameType {
    /// 游戏模式序数。
    ///
    /// Game mode ordinal.
    pub gamemode: i32,
}

impl Writer for SetPlayerGameType {
    /// 写入 SetPlayerGameType 包体（单个 zigzag varint32）。
    ///
    /// Writes the SetPlayerGameType body (a single zigzag varint32).
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_i32(self.gamemode)
    }
}

impl Reader<SetPlayerGameType> for SetPlayerGameType {
    /// 不支持解码：服务端只发送游戏模式设置。
    ///
    /// Decoding is unsupported: the server only sends game mode changes.
    ///
    /// # Errors
    /// 始终返回 [`std::io::ErrorKind::Unsupported`] 错误。
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
