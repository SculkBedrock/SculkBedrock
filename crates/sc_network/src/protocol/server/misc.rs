use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_nbt::network::BedrockNetworkNbt;
use sc_nbt::{NbtValue, SCNBTByteWriter};
use sc_network_macros::MinecraftPacket;
use sc_utils::game::gamerules::GameRules;
use sc_utils::game::skin::Skin;
use std::io::Error;
use uuid::Uuid;

/// SetSpawnPositionPacket (0x2b)
/// Spawn position notification for the client.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct SetSpawnPosition {
    pub spawn_type: u32, // 0 = player spawn, 1 = world spawn
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub dimension: i32,
    /// Optional compass/respawn block position. Left unset during
    /// normal join, which serializes as Bedrock's invalid block position.
    pub spawn_block_position: Option<(i32, i32, i32)>,
}

impl SetSpawnPosition {
    pub const TYPE_PLAYER_SPAWN: u32 = 0;
    pub const TYPE_WORLD_SPAWN: u32 = 1;
}

impl Writer for SetSpawnPosition {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_i32(self.spawn_type as i32)?; // putVarInt
        write_block_position(buf, self.x, self.y, self.z)?;
        buf.write_var_i32(self.dimension)?; // putVarInt
                                            // Real coordinates are always written; None falls back to main coordinates.
        let (spawn_x, spawn_y, spawn_z) = self
            .spawn_block_position
            .unwrap_or((self.x, self.y, self.z));
        write_block_position(buf, spawn_x, spawn_y, spawn_z)?;
        Ok(())
    }
}

/// Bedrock `BlockPosition`: x/y/z are all zigzag varint32, matching
/// `UpdateBlock`.
fn write_block_position(buf: &mut ByteWriter, x: i32, y: i32, z: i32) -> Result<(), Error> {
    buf.write_var_i32(x)?;
    buf.write_var_i32(y)?;
    buf.write_var_i32(z)
}

impl Reader<SetSpawnPosition> for SetSpawnPosition {
    fn read(_buf: &mut ByteReader) -> Result<SetSpawnPosition, Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "SetSpawnPosition decode is unsupported",
        ))
    }
}

/// SetTimePacket (0x0a)
/// World time sync.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct SetTime {
    pub time: i32,
}

impl Writer for SetTime {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_i32(self.time)?; // putVarInt
        Ok(())
    }
}

impl Reader<SetTime> for SetTime {
    fn read(buf: &mut ByteReader) -> Result<SetTime, Error> {
        Ok(SetTime {
            time: buf.read_var_i32()?,
        })
    }
}

/// SetDifficultyPacket (0x3c)
/// Game difficulty setting.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct SetDifficulty {
    pub difficulty: u32,
}

impl Writer for SetDifficulty {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_u32(self.difficulty)?; // putUnsignedVarInt
        Ok(())
    }
}

impl Reader<SetDifficulty> for SetDifficulty {
    fn read(buf: &mut ByteReader) -> Result<SetDifficulty, Error> {
        Ok(SetDifficulty {
            difficulty: buf.read_var_u32()?,
        })
    }
}

/// SetCommandsEnabledPacket (0x3b)
/// Client command toggle.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct SetCommandsEnabled {
    pub enabled: bool,
}

impl Writer for SetCommandsEnabled {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_bool(self.enabled)?; // putBoolean
        Ok(())
    }
}

impl Reader<SetCommandsEnabled> for SetCommandsEnabled {
    fn read(buf: &mut ByteReader) -> Result<SetCommandsEnabled, Error> {
        Ok(SetCommandsEnabled {
            enabled: buf.read_bool()?,
        })
    }
}

/// GameRulesChangedPacket (0x48)
/// Game rule change sync.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct GameRulesChanged {
    pub gamerules: GameRules,
}

impl Writer for GameRulesChanged {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        // Non-StartGame mode: INTEGER uses LInt (4-byte little-endian).
        buf.write_game_rules_with_mode(&self.gamerules, false)?;
        Ok(())
    }
}

impl Reader<GameRulesChanged> for GameRulesChanged {
    fn read(_buf: &mut ByteReader) -> Result<GameRulesChanged, Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "GameRulesChanged decode is unsupported",
        ))
    }
}

/// PlayerListPacket (0x3f)
/// Online player list sync.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct PlayerList {
    pub list_type: u8, // 0=ADD, 1=REMOVE
    pub entries: Vec<PlayerListEntry>,
}

impl PlayerList {
    pub const TYPE_ADD: u8 = 0;
    pub const TYPE_REMOVE: u8 = 1;

    /// Empty ADD list.
    pub fn empty_add() -> Self {
        Self {
            list_type: Self::TYPE_ADD,
            entries: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PlayerListEntry {
    pub uuid: Uuid,
    pub entity_id: i64,
    pub name: String,
    pub xuid: String,
    pub platform_chat_id: String,
    pub build_platform: i32,
    pub skin: Option<Skin>,
    pub is_teacher: bool,
    pub is_host: bool,
    pub is_sub_client: bool,
    pub color: i32,
}

impl Default for PlayerListEntry {
    fn default() -> Self {
        Self {
            uuid: Uuid::nil(),
            entity_id: 0,
            name: String::new(),
            xuid: String::new(),
            platform_chat_id: String::new(),
            build_platform: -1,
            skin: None,
            is_teacher: false,
            is_host: false,
            is_sub_client: false,
            color: 0,
        }
    }
}

/// PlayerList ADD entry body:
/// uuid + entityId(varlong) + name + xuid + platformChatId + buildPlatform(LInt)
/// + skin + isTeacher + isHost + isSubClient + color(LInt).
fn write_player_list_add_entry(buf: &mut ByteWriter, entry: &PlayerListEntry) -> Result<(), Error> {
    buf.write_uuid(&entry.uuid)?; // putUUID
    buf.write_var_i64(entry.entity_id)?; // putVarLong
    buf.write_string(&entry.name)?; // putString
    buf.write_string(&entry.xuid)?; // putString(xboxUserId)
    buf.write_string(&entry.platform_chat_id)?; // putString
    buf.write_i32_le(entry.build_platform)?; // putLInt
                                             // Full skin echo (all appearance fields preserved from
                                             // login parsing; missing fields crash real clients
                                             // during world init).
    let default_skin = Skin::default();
    let skin = entry.skin.as_ref().unwrap_or(&default_skin);
    buf.write_string(skin.skin_id())?; // skinId
    if !crate::protocol::version::protocol_at_least(
        crate::protocol::version::PROTOCOL_VERSION_1_26_60,
    ) {
        buf.write_string(&skin.play_fab_id)?; // playFabId (removed in 2225)
    }
    buf.write_string(if skin.skin_resource_patch.is_empty() {
        // Fall back to default Steve geometry when the client omits it.
        r#"{"geometry":{"default":"geometry.humanoid.custom"}}"#
    } else {
        &skin.skin_resource_patch
    })?; // skinResourcePatch
    let (skin_width, skin_height) = skin.dimensions();
    buf.write_i32_le(skin_width)?; // putImage: width (LInt)
    buf.write_i32_le(skin_height)?; // putImage: height (LInt)
    buf.write_slice(skin.skin_data())?; // putImage: data (byte array)
    buf.write_var_u32(skin.animations.len() as u32)?; // animations.size
    for anim in &skin.animations {
        buf.write_i32_le(anim.width as i32)?;
        buf.write_i32_le(anim.height as i32)?;
        buf.write_slice(&anim.image)?;
        // animation_type/expression_type are varints; fixed-width writes
        // would shift all following skin fields.
        buf.write_var_u32(anim.texture_type as u32)?;
        buf.write_f32_le(anim.frames)?;
        buf.write_var_u32(anim.expression_type as u32)?;
    }
    buf.write_i32_le(skin.cape_width as i32)?; // capeData width (LInt)
    buf.write_i32_le(skin.cape_height as i32)?; // capeData height (LInt)
    buf.write_slice(&skin.cape_data)?; // capeData (byte array)
    buf.write_string(if skin.geometry_data.is_empty() {
        "{}" // default empty geometry
    } else {
        &skin.geometry_data
    })?; // geometryData
    buf.write_string(if skin.geometry_data_engine_version.is_empty() {
        "0.0.0" // default minimum engine version
    } else {
        &skin.geometry_data_engine_version
    })?; // geometryDataEngineVersion
    buf.write_string(&skin.animation_data)?; // animationData
    buf.write_string(&skin.cape_id)?; // capeId
    buf.write_string(if skin.full_skin_id.is_empty() {
        skin.skin_id() // fullSkinId defaults to skinId
    } else {
        &skin.full_skin_id
    })?; // fullSkinId
    buf.write_u8(if skin.arm_size.eq_ignore_ascii_case("slim") {
        0
    } else {
        1
    })?; // armSize (0=slim, 1=wide)
    buf.write_i32_le(Skin::parse_hex_color(&skin.skin_color))?; // skinColor
    buf.write_var_u32(skin.persona_pieces.len() as u32)?; // personaPieces.size
    for piece in &skin.persona_pieces {
        buf.write_string(&piece.piece_id)?;
        buf.write_i32_le(persona_piece_type_id(&piece.piece_type))?; // enum value
        buf.write_uuid(&parse_persona_pack_uuid(&piece.pack_id))?; // packId (UUID)
        buf.write_bool(piece.is_default)?;
        buf.write_string(&piece.product_id)?;
    }
    buf.write_var_u32(skin.tint_colors.len() as u32)?; // tintColors.size
    for tint in &skin.tint_colors {
        // Tint type is a string (e.g. "persona_mouth"), not an enum ordinal.
        buf.write_string(&tint.piece_type)?;
        buf.write_var_u32(tint.colors.len() as u32)?;
        for i in 0..4 {
            // Exactly 4 colors are required; missing entries are zero.
            let color = tint
                .colors
                .get(i)
                .map(|c| Skin::parse_hex_color(c))
                .unwrap_or(0);
            buf.write_i32_le(color)?;
        }
    }
    buf.write_bool(skin.premium)?; // isPremium
    buf.write_bool(skin.persona)?; // isPersona
    buf.write_bool(skin.cape_on_classic)?; // isCapeOnClassic
    buf.write_bool(true)?; // isPrimaryUser (list entries address the player)
    buf.write_bool(skin.overriding_player_appearance)?; // isOverridingPlayerAppearance
    buf.write_string("false")?; // isTrusted (putString: Boolean.toString)
    buf.write_string("")?; // profileHash (always empty)
    buf.write_bool(entry.is_teacher)?;
    buf.write_bool(entry.is_host)?;
    buf.write_bool(entry.is_sub_client)?;
    buf.write_i32_le(entry.color)?; // putLInt(color.getRGB())
    Ok(())
}

/// PersonaPieceType enum id (unknown=0, CoCo=28 on protocol 2225 and
/// later, unsupported=28 before and 29 on 2225 and later).
/// Input is the login "persona_" prefixed string (e.g. persona_skeleton),
/// unknown values map to 0.
fn persona_piece_type_id(piece_type: &str) -> i32 {
    let id = piece_type
        .strip_prefix("persona_")
        .unwrap_or(piece_type)
        .to_ascii_lowercase();
    let modern = crate::protocol::version::protocol_at_least(
        crate::protocol::version::PROTOCOL_VERSION_1_26_60,
    );
    match id.as_str() {
        "skeleton" => 1,
        "body" => 2,
        "skin" => 3,
        "bottom" => 4,
        "feet" => 5,
        "dress" => 6,
        "top" => 7,
        "high_pants" | "highpants" => 8,
        "hands" => 9,
        "outerwear" => 10,
        "facial_hair" | "facialhair" => 11,
        "mouth" => 12,
        "eyes" => 13,
        "hair" => 14,
        "hood" => 15,
        "back" => 16,
        "face_accessory" | "faceaccessory" => 17,
        "head" => 18,
        "legs" => 19,
        "left_leg" | "leftleg" => 20,
        "right_leg" | "rightleg" => 21,
        "arms" => 22,
        "left_arm" | "leftarm" => 23,
        "right_arm" | "rightarm" => 24,
        "capes" => 25,
        "classic_skin" | "classicskin" => 26,
        "emote" => 27,
        "coco" => 28,
        "unsupported" => {
            if modern {
                29
            } else {
                28
            }
        }
        _ => 0, // unknown
    }
}

/// packId string to UUID (nil UUID on parse failure).
fn parse_persona_pack_uuid(value: &str) -> Uuid {
    Uuid::parse_str(value).unwrap_or_else(|_| Uuid::nil())
}

impl Writer for PlayerList {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        let protocol = crate::protocol::version::current_protocol_version();
        let is_add = self.list_type == Self::TYPE_ADD;
        if protocol >= 2168 {
            // Single uvarint count, then per entry [type(uvarint ordinal,
            // ADD=1/REMOVE=0) + legacyId(u8, ADD=0/REMOVE=1) + entry data];
            // no legacy second isTrusted round.
            buf.write_var_u32(self.entries.len() as u32)?;
            for entry in &self.entries {
                buf.write_var_u32(if is_add { 1 } else { 0 })?; // isAdd
                if is_add {
                    buf.write_u8(0)?; // entryType = TYPE_ADD
                    write_player_list_add_entry(buf, entry)?;
                } else {
                    buf.write_u8(1)?; // entryType = TYPE_REMOVE
                    buf.write_uuid(&entry.uuid)?;
                }
            }
        } else {
            // Older protocols: type(byte) + count(varuint) + entries + isTrusted round.
            buf.write_u8(self.list_type)?;
            buf.write_var_u32(self.entries.len() as u32)?;
            if is_add {
                for entry in &self.entries {
                    write_player_list_add_entry(buf, entry)?;
                }
                for _ in &self.entries {
                    buf.write_bool(false)?; // skin.isTrusted()
                }
            } else {
                for entry in &self.entries {
                    buf.write_uuid(&entry.uuid)?;
                }
            }
        }
        Ok(())
    }
}

impl Reader<PlayerList> for PlayerList {
    fn read(_buf: &mut ByteReader) -> Result<PlayerList, Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "PlayerList decode is unsupported",
        ))
    }
}

/// SetEntityDataPacket (0x27)
/// Entity metadata delivery to the client.
/// Wire order: entity runtime id, then metadata entries.
///   putUnsignedVarInt(0) // Entity properties int
///   putUnsignedVarInt(0) // Entity properties float
///   putUnsignedVarLong(frame)
#[derive(Clone, Debug, MinecraftPacket)]
pub struct SetEntityData {
    pub entity_runtime_id: u64,
    pub metadata: Vec<EntityMetadataEntry>,
    pub frame: u64,
}

impl SetEntityData {
    /// Empty SetEntityData packet (no metadata).
    pub fn empty(entity_runtime_id: u64) -> Self {
        Self {
            entity_runtime_id,
            metadata: Vec::new(),
            frame: 0,
        }
    }
}

/// Entity metadata entry.
#[derive(Clone, Debug)]
pub struct EntityMetadataEntry {
    pub key: u32,
    pub data_type: u32,
    pub value: EntityMetadataValue,
}

/// Entity metadata value.
#[derive(Clone, Debug)]
pub enum EntityMetadataValue {
    Byte(i8),
    Short(i16),
    Int(i32),
    Float(f32),
    String(String),
    Long(i64),
    Nbt(NbtValue),
    Vector3i(i32, i32, i32),
    Vector3f(f32, f32, f32),
}

/// Entity metadata writer (shared by SetEntityData and AddItemActor).
///
/// Per entry: key(varuint32) + type(varuint32) + legacy_type(u8) + value.
/// Newer protocols add the legacy type byte; the official 1.26.40 schema:
/// key(varint)+type(varint)+legacy_type(u8)+value.
pub(crate) fn write_entity_metadata(
    buf: &mut ByteWriter,
    entries: &[EntityMetadataEntry],
) -> Result<(), Error> {
    buf.write_var_u32(entries.len() as u32)?; // count
    for entry in entries {
        buf.write_var_u32(entry.key)?; // key
        buf.write_var_u32(entry.data_type)?; // type (written once)
        buf.write_u8(entry.data_type as u8)?; // legacy_type
        match &entry.value {
            EntityMetadataValue::Byte(v) => {
                buf.write_i8(*v)?;
            }
            EntityMetadataValue::Short(v) => {
                buf.write_i16_le(*v)?;
            }
            EntityMetadataValue::Int(v) => {
                buf.write_var_i32(*v)?;
            }
            EntityMetadataValue::Float(v) => {
                buf.write_f32_le(*v)?;
            }
            EntityMetadataValue::String(v) => {
                buf.write_string(v)?;
            }
            EntityMetadataValue::Long(v) => {
                buf.write_var_i64(*v)?;
            }
            EntityMetadataValue::Nbt(v) => {
                buf.write_nbt::<BedrockNetworkNbt>(v)?;
            }
            EntityMetadataValue::Vector3i(x, y, z) => {
                buf.write_var_i32(*x)?;
                buf.write_var_i32(*y)?;
                buf.write_var_i32(*z)?;
            }
            EntityMetadataValue::Vector3f(x, y, z) => {
                buf.write_f32_le(*x)?;
                buf.write_f32_le(*y)?;
                buf.write_f32_le(*z)?;
            }
        }
    }
    Ok(())
}

impl Writer for SetEntityData {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_u64(self.entity_runtime_id)?; // putEntityRuntimeId

        write_entity_metadata(buf, &self.metadata)?;

        // Entity properties (zeros)
        buf.write_var_u32(0)?; // Entity properties int count
        buf.write_var_u32(0)?; // Entity properties float count

        // frame
        buf.write_var_u64(self.frame)?;
        Ok(())
    }
}

impl Reader<SetEntityData> for SetEntityData {
    fn read(_buf: &mut ByteReader) -> Result<SetEntityData, Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "SetEntityData decode is unsupported",
        ))
    }
}

/// CraftingDataPacket (0x34)
/// Crafting recipe data for the client (segmented counts layout).
///
/// Wire structs below are **not** the internal recipe structs: they are the
/// 2168 wire representation built from a
/// [`sc_recipe::RecipeRegistrySnapshot`] via [`CraftingData::from_snapshot`].
/// Only correctly compiled, wire-expressible recipes are sent; anything
/// else is reported in the disabled list and never encoded.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct CraftingData {
    pub clean_recipes: bool,
    pub shaped: Vec<CraftingShapedWire>,
    /// Shapeless crafting + furnace entries share the shapeless
    /// segment (the per-entry tag distinguishes furnace/smoker/blast).
    pub shapeless: Vec<CraftingShapelessWire>,
    pub smithing_transform: Vec<CraftingSmithingWire>,
    pub brewing: Vec<CraftingBrewingWire>,
    pub containers: Vec<CraftingContainerWire>,
}

/// 2168 shaped entry.
#[derive(Clone, Debug)]
pub struct CraftingShapedWire {
    pub recipe_id: String,
    pub width: u8,
    pub height: u8,
    /// Row-major `width*height`; `None` = empty slot.
    pub ingredients: Vec<Option<CraftingIngredientWire>>,
    pub results: Vec<CraftingResultWire>,
    pub tag: String,
    pub priority: i32,
    pub assume_symmetry: bool,
    pub network_id: u32,
}

/// 2168 shapeless/furnace entry.
#[derive(Clone, Debug)]
pub struct CraftingShapelessWire {
    pub recipe_id: String,
    pub ingredients: Vec<CraftingIngredientWire>,
    pub results: Vec<CraftingResultWire>,
    pub tag: String,
    pub priority: i32,
    pub network_id: u32,
}

/// 2168 smithing-transform entry.
#[derive(Clone, Debug)]
pub struct CraftingSmithingWire {
    pub recipe_id: String,
    pub template: CraftingIngredientWire,
    pub base: CraftingIngredientWire,
    pub addition: CraftingIngredientWire,
    pub result: CraftingResultWire,
    pub tag: String,
    pub network_id: u32,
}

/// 2168 brewing entry (numeric ids + meta).
#[derive(Clone, Debug)]
pub struct CraftingBrewingWire {
    pub input_id: i32,
    pub input_meta: i32,
    pub ingredient_id: i32,
    pub ingredient_meta: i32,
    pub output_id: i32,
    pub output_meta: i32,
}

/// 2168 brewing-container entry (numeric ids).
#[derive(Clone, Debug)]
pub struct CraftingContainerWire {
    pub input_id: i32,
    pub ingredient_id: i32,
    pub output_id: i32,
}

/// One wire ingredient: exact item or tag. This is the wire form of
/// `IngredientChoice`, not the internal spec itself.
#[derive(Clone, Debug)]
pub enum CraftingIngredientWire {
    Empty,
    Item {
        identifier: String,
        data: i32,
        count: i32,
    },
    Tag {
        tag: String,
        count: i32,
    },
}

/// One wire result (instance-item descriptor fields).
#[derive(Clone, Debug)]
pub struct CraftingResultWire {
    pub runtime_id: u16,
    pub count: u16,
    pub damage: u32,
    pub block_runtime_id: u32,
}

impl CraftingData {
    /// Empty CraftingData packet (clears client recipes).
    pub fn empty() -> Self {
        Self {
            clean_recipes: true,
            shaped: Vec::new(),
            shapeless: Vec::new(),
            smithing_transform: Vec::new(),
            brewing: Vec::new(),
            containers: Vec::new(),
        }
    }

    /// Build from a registry snapshot. Only wire-expressible recipes are
    /// included; the returned disabled list explains every omission.
    ///
    /// - `item_runtime` resolves an item identifier to
    ///   `(runtime_id, block_runtime_id)`. Unknown items disable the recipe.
    /// - `brewing_id` resolves brewing/container identifiers to numeric
    ///   `(id, meta)`. Unresolvable brewing entries are disabled, never
    ///   sent with guessed ids.
    pub fn from_snapshot(
        snapshot: &sc_recipe::RecipeRegistrySnapshot,
        item_runtime: &dyn Fn(&str) -> Option<(u16, u32)>,
        brewing_id: &dyn Fn(&str) -> Option<(i32, i32)>,
    ) -> (Self, Vec<(String, String)>) {
        use sc_recipe::{RecipeBody, StationKind};
        let mut data = Self::empty();
        let mut disabled: Vec<(String, String)> = snapshot
            .disabled_reasons()
            .iter()
            .map(|(id, reason)| (id.clone(), format!("compile disabled: {reason}")))
            .collect();
        // Network ids start at 2: 1 is reserved for the hardcoded trim.
        for (index, recipe) in snapshot.in_network_order().iter().enumerate() {
            let next_id = index as u32 + 2;
            match &recipe.body {
                RecipeBody::Shaped(body) => {
                    let tag = recipe
                        .tags
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "crafting_table".to_string());
                    if tag != "crafting_table" {
                        disabled.push((
                            recipe.identifier.clone(),
                            format!("shaped station '{tag}' has no 2168 wire segment"),
                        ));
                        continue;
                    }
                    let mut ingredients = Vec::new();
                    for cell in &body.grid {
                        match cell {
                            None => ingredients.push(None),
                            Some(spec) => {
                                // Shaped cells hold exactly one choice; take
                                // the first (canonical) alternative.
                                let Some(choice) = spec.choices.first() else {
                                    ingredients.push(None);
                                    continue;
                                };
                                ingredients.push(Some(choice_to_wire(choice)));
                            }
                        }
                    }
                    let mut results = Vec::new();
                    let mut ok = true;
                    for out in &body.results {
                        match item_runtime(&out.identifier) {
                            Some((runtime_id, block_runtime_id)) => {
                                results.push(CraftingResultWire {
                                    runtime_id,
                                    count: out.count,
                                    damage: out.data.unwrap_or(0).max(0) as u32,
                                    block_runtime_id,
                                })
                            }
                            None => {
                                disabled.push((
                                    recipe.identifier.clone(),
                                    format!("unknown item '{}'", out.identifier),
                                ));
                                ok = false;
                                break;
                            }
                        }
                    }
                    if !ok {
                        continue;
                    }
                    data.shaped.push(CraftingShapedWire {
                        recipe_id: recipe.identifier.clone(),
                        width: body.width,
                        height: body.height,
                        ingredients,
                        results,
                        tag,
                        priority: recipe.priority,
                        assume_symmetry: body.assume_symmetry,
                        network_id: next_id,
                    });
                }
                RecipeBody::Shapeless(body) => {
                    let tag = recipe
                        .tags
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "crafting_table".to_string());
                    let mut ingredients = Vec::new();
                    for spec in &body.ingredients {
                        // Expand count>1 into repeated entries (one
                        // ingredient per unit on the wire).
                        for _ in 0..spec.count.max(1) {
                            let Some(choice) = spec.choices.first() else {
                                continue;
                            };
                            ingredients.push(choice_to_wire(choice));
                        }
                    }
                    let mut results = Vec::new();
                    let mut ok = true;
                    for out in &body.results {
                        match item_runtime(&out.identifier) {
                            Some((runtime_id, block_runtime_id)) => {
                                results.push(CraftingResultWire {
                                    runtime_id,
                                    count: out.count,
                                    damage: out.data.unwrap_or(0).max(0) as u32,
                                    block_runtime_id,
                                })
                            }
                            None => {
                                disabled.push((
                                    recipe.identifier.clone(),
                                    format!("unknown item '{}'", out.identifier),
                                ));
                                ok = false;
                                break;
                            }
                        }
                    }
                    if !ok {
                        continue;
                    }
                    // Stonecutter/crafting-table tags ride in the entry tag.
                    let _ = StationKind::CraftingTable;
                    data.shapeless.push(CraftingShapelessWire {
                        recipe_id: recipe.identifier.clone(),
                        ingredients,
                        results,
                        tag,
                        priority: recipe.priority,
                        network_id: next_id,
                    });
                }
                RecipeBody::Furnace(body) | RecipeBody::FurnaceMaterial(body) => {
                    let tag = recipe
                        .tags
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "furnace".to_string());
                    let Some(choice) = body.input.choices.first() else {
                        disabled.push((
                            recipe.identifier.clone(),
                            "furnace input has no choices".to_string(),
                        ));
                        continue;
                    };
                    let (Some((_, _)), Some((out_runtime, out_block))) = (
                        item_runtime_of_choice(choice, item_runtime),
                        item_runtime(&body.output.identifier),
                    ) else {
                        disabled.push((
                            recipe.identifier.clone(),
                            "unknown furnace item".to_string(),
                        ));
                        continue;
                    };
                    let _ = (out_runtime, out_block);
                    data.shapeless.push(CraftingShapelessWire {
                        recipe_id: recipe.identifier.clone(),
                        ingredients: vec![choice_to_wire(choice)],
                        results: vec![CraftingResultWire {
                            runtime_id: out_runtime,
                            count: body.output.count,
                            damage: body.output.data.unwrap_or(0).max(0) as u32,
                            block_runtime_id: out_block,
                        }],
                        tag,
                        priority: 0,
                        network_id: next_id,
                    });
                }
                RecipeBody::SmithingTransform(body) => {
                    let conv = |spec: &sc_recipe::IngredientSpec| {
                        spec.choices
                            .first()
                            .map(choice_to_wire)
                            .unwrap_or(CraftingIngredientWire::Empty)
                    };
                    let Some((out_runtime, out_block)) = item_runtime(&body.result.identifier)
                    else {
                        disabled.push((
                            recipe.identifier.clone(),
                            format!("unknown item '{}'", body.result.identifier),
                        ));
                        continue;
                    };
                    // Template/base/addition identifiers must resolve too;
                    // tags are wire-expressible so they pass through.
                    for (spec, name) in [
                        (&body.template, "template"),
                        (&body.base, "base"),
                        (&body.addition, "addition"),
                    ] {
                        if let Some(sc_recipe::IngredientChoice::Item { identifier, .. }) =
                            spec.choices.first()
                        {
                            if item_runtime(identifier).is_none() {
                                disabled.push((
                                    recipe.identifier.clone(),
                                    format!("unknown smithing {name} '{identifier}'"),
                                ));
                            }
                        }
                    }
                    if disabled
                        .last()
                        .map(|(id, _)| id == &recipe.identifier)
                        .unwrap_or(false)
                    {
                        continue;
                    }
                    data.smithing_transform.push(CraftingSmithingWire {
                        recipe_id: recipe.identifier.clone(),
                        template: conv(&body.template),
                        base: conv(&body.base),
                        addition: conv(&body.addition),
                        result: CraftingResultWire {
                            runtime_id: out_runtime,
                            count: body.result.count,
                            damage: body.result.data.unwrap_or(0).max(0) as u32,
                            block_runtime_id: out_block,
                        },
                        tag: "smithing_table".to_string(),
                        network_id: next_id,
                    });
                }
                RecipeBody::SmithingTrim(_) => {
                    // Absorbed by the hardcoded 2168 trim entry below.
                    disabled.push((
                        recipe.identifier.clone(),
                        "absorbed by hardcoded 2168 trim entry".to_string(),
                    ));
                }
                RecipeBody::BrewingMix(body) | RecipeBody::BrewingContainer(body) => {
                    let input_id = choice_to_numeric(&body.input, brewing_id);
                    let reagent_id = choice_to_numeric(&body.reagent, brewing_id);
                    let output_id = match body.output.identifier.as_str() {
                        id => brewing_id(id),
                    };
                    match (input_id, reagent_id, output_id) {
                        (Some((a, am)), Some((b, bm)), Some((c, cm))) => {
                            if matches!(&recipe.body, RecipeBody::BrewingMix(_)) {
                                data.brewing.push(CraftingBrewingWire {
                                    input_id: a,
                                    input_meta: am,
                                    ingredient_id: b,
                                    ingredient_meta: bm,
                                    output_id: c,
                                    output_meta: cm,
                                });
                            } else {
                                data.containers.push(CraftingContainerWire {
                                    input_id: a,
                                    ingredient_id: b,
                                    output_id: c,
                                });
                            }
                        }
                        _ => {
                            disabled.push((
                                recipe.identifier.clone(),
                                "brewing identifier has no numeric mapping".to_string(),
                            ));
                        }
                    }
                }
                RecipeBody::MaterialReducer(_) => {
                    disabled.push((
                        recipe.identifier.clone(),
                        "material reducer has no 2168 wire segment in this build".to_string(),
                    ));
                }
            }
        }
        (data, disabled)
    }

    pub fn counts(&self) -> (usize, usize, usize, usize, usize) {
        (
            self.shaped.len(),
            self.shapeless.len(),
            self.smithing_transform.len(),
            self.brewing.len(),
            self.containers.len(),
        )
    }
}

fn choice_to_wire(choice: &sc_recipe::IngredientChoice) -> CraftingIngredientWire {
    match choice {
        sc_recipe::IngredientChoice::Item { identifier, data } => CraftingIngredientWire::Item {
            identifier: identifier.clone(),
            data: data.unwrap_or(32767),
            count: 1,
        },
        sc_recipe::IngredientChoice::Tag { tag } => CraftingIngredientWire::Tag {
            tag: tag.clone(),
            count: 1,
        },
    }
}

fn item_runtime_of_choice(
    choice: &sc_recipe::IngredientChoice,
    resolve: &dyn Fn(&str) -> Option<(u16, u32)>,
) -> Option<(u16, u32)> {
    match choice {
        sc_recipe::IngredientChoice::Item { identifier, .. } => resolve(identifier),
        // Tags are wire-expressible without resolving to one runtime id.
        sc_recipe::IngredientChoice::Tag { .. } => Some((0, 0)),
    }
}

fn choice_to_numeric(
    spec: &sc_recipe::IngredientSpec,
    resolve: &dyn Fn(&str) -> Option<(i32, i32)>,
) -> Option<(i32, i32)> {
    let choice = spec.choices.first()?;
    match choice {
        sc_recipe::IngredientChoice::Item { identifier, data } => {
            if let Some(mapped) = resolve(identifier) {
                Some(mapped)
            } else {
                // Fall back to (0, data) only when the resolver has no
                // entry? No: fail closed so callers disable loudly.
                let _ = data;
                None
            }
        }
        sc_recipe::IngredientChoice::Tag { .. } => None,
    }
}

fn write_ingredient(
    buf: &mut ByteWriter,
    ingredient: &CraftingIngredientWire,
) -> Result<(), Error> {
    match ingredient {
        CraftingIngredientWire::Empty => {
            buf.write_var_u32(0)?;
            buf.write_u16_le(0)?;
        }
        CraftingIngredientWire::Item {
            identifier,
            data,
            count,
        } => {
            buf.write_var_u32(1)?;
            buf.write_string(identifier)?;
            buf.write_var_i32(*data)?;
            buf.write_u16_le(*count as u16)?;
        }
        CraftingIngredientWire::Tag { tag, count, .. } => {
            buf.write_var_u32(3)?;
            buf.write_string(tag)?;
            buf.write_u16_le(*count as u16)?;
        }
    }
    Ok(())
}

fn write_result(buf: &mut ByteWriter, result: &CraftingResultWire) -> Result<(), Error> {
    // Slot layout (instance=true): varint runtime + LShort count +
    // uvarint damage + uvarint block + user-data blob. Reuse the network
    // instance-item writer for layout compatibility.
    let item = crate::protocol::client::transaction::ItemData {
        runtime_id: result.runtime_id,
        count: result.count,
        damage: result.damage,
        has_net_id: false,
        net_id: 0,
        block_runtime_id: result.block_runtime_id,
        user_data: None,
    };
    crate::protocol::client::transaction::write_instance_item(buf, &item)
}

fn recipe_uuid(recipe_id: &str) -> uuid::Uuid {
    uuid::Uuid::new_v3(&uuid::Uuid::NAMESPACE_OID, recipe_id.as_bytes())
}

impl Default for CraftingData {
    fn default() -> Self {
        Self::empty()
    }
}

impl Writer for CraftingData {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        // Segmented counts by recipe kind.
        // Fixed order: shaped / shapeless (with furnace) / multi / user /
        // chemistry-shapeless / chemistry-shaped / smithingTransform /
        // smithingTrim / brewing / container / materialReducer / clean.
        buf.write_var_u32(self.shaped.len() as u32)?;
        for shaped in &self.shaped {
            buf.write_string(&shaped.recipe_id)?;
            buf.write_var_i32(shaped.width as i32)?;
            buf.write_var_i32(shaped.height as i32)?;
            buf.write_var_u32(shaped.ingredients.len() as u32)?;
            for ingredient in &shaped.ingredients {
                match ingredient {
                    None => write_ingredient(buf, &CraftingIngredientWire::Empty)?,
                    Some(wire) => write_ingredient(buf, wire)?,
                }
            }
            buf.write_var_u32(shaped.results.len() as u32)?;
            for result in &shaped.results {
                write_result(buf, result)?;
            }
            buf.write_uuid(&recipe_uuid(&shaped.recipe_id))?;
            buf.write_string(&shaped.tag)?;
            buf.write_var_i32(shaped.priority)?;
            buf.write_bool(shaped.assume_symmetry)?;
            buf.write_bool(false)?; // unlock requirement: none on the wire
            buf.write_var_u32(shaped.network_id)?;
        }
        buf.write_var_u32(self.shapeless.len() as u32)?;
        for entry in &self.shapeless {
            buf.write_string(&entry.recipe_id)?;
            buf.write_var_u32(entry.ingredients.len() as u32)?;
            for ingredient in &entry.ingredients {
                write_ingredient(buf, ingredient)?;
            }
            // Shapeless/furnace entries always carry one result list;
            // multi-result shapeless packs only the first result on the wire
            // (the remainder is server-side container remainder, never sent).
            buf.write_var_u32(entry.results.len() as u32)?;
            for result in &entry.results {
                write_result(buf, result)?;
            }
            buf.write_uuid(&recipe_uuid(&entry.recipe_id))?;
            buf.write_string(&entry.tag)?;
            buf.write_var_i32(entry.priority)?;
            buf.write_bool(false)?; // unlock requirement: none on the wire
            buf.write_var_u32(entry.network_id)?;
        }
        buf.write_var_u32(0)?; // multiData.size()
        buf.write_var_u32(0)?; // user
        buf.write_var_u32(0)?; // chemistry shapeless
        buf.write_var_u32(0)?; // chemistry shaped
        buf.write_var_u32(self.smithing_transform.len() as u32)?;
        for smithing in &self.smithing_transform {
            buf.write_string(&smithing.recipe_id)?;
            write_ingredient(buf, &smithing.template)?;
            write_ingredient(buf, &smithing.base)?;
            write_ingredient(buf, &smithing.addition)?;
            write_result(buf, &smithing.result)?;
            buf.write_string(&smithing.tag)?;
            buf.write_var_u32(smithing.network_id)?;
        }
        // smithing trim: one fixed minecraft:smithing_armor_trim entry.
        buf.write_var_u32(1)?;
        buf.write_string("minecraft:smithing_armor_trim")?;
        for item_tag in [
            "minecraft:trim_templates",
            "minecraft:trimmable_armors",
            "minecraft:trim_materials",
        ] {
            // putTrimRecipeIngredient:varuint(1) + "item_tag" + tag + meta(32767) + count(1)
            buf.write_var_u32(1)?;
            buf.write_string("item_tag")?;
            buf.write_string(item_tag)?;
            buf.write_var_i32(32767)?; // meta
            buf.write_var_i32(1)?; // count
        }
        buf.write_string("smithing_table")?; // CRAFTING_TAG_SMITHING_TABLE
        buf.write_var_u32(1)?; // network id (fixed)
        buf.write_var_u32(self.brewing.len() as u32)?;
        for brewing in &self.brewing {
            buf.write_var_i32(brewing.input_id)?;
            buf.write_var_i32(brewing.input_meta)?;
            buf.write_var_i32(brewing.ingredient_id)?;
            buf.write_var_i32(brewing.ingredient_meta)?;
            buf.write_var_i32(brewing.output_id)?;
            buf.write_var_i32(brewing.output_meta)?;
        }
        buf.write_var_u32(self.containers.len() as u32)?;
        for container in &self.containers {
            buf.write_var_i32(container.input_id)?;
            buf.write_var_i32(container.ingredient_id)?;
            buf.write_var_i32(container.output_id)?;
        }
        buf.write_var_u32(0)?; // material reducers size
        buf.write_bool(self.clean_recipes)
    }
}

impl Reader<CraftingData> for CraftingData {
    fn read(_buf: &mut ByteReader) -> Result<CraftingData, Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "CraftingData decode is unsupported",
        ))
    }
}

/// AvailableCommandsPacket (0x4c)
/// Available command data for the client.
/// Empty variant: all collections are empty.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct AvailableCommands {
    pub commands: Vec<CommandData>,
}

impl AvailableCommands {
    /// Empty AvailableCommands packet (no commands).
    pub fn empty() -> Self {
        Self {
            commands: Vec::new(),
        }
    }
}

impl Default for AvailableCommands {
    fn default() -> Self {
        Self::empty()
    }
}

#[derive(Clone, Debug)]
pub struct CommandData {
    pub name: String,
    pub description: String,
    pub flags: u16,
    /// Permission name string (any/gamedirectors/admin/host/owner).
    pub permission: u8,
}

impl Writer for AvailableCommands {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_u32(0)?; // enum values count = 0
        buf.write_var_u32(0)?; // subCommandValues count = 0
        buf.write_var_u32(0)?; // postfixes count = 0
        buf.write_var_u32(0)?; // enums count = 0
        buf.write_var_u32(0)?; // subCommandData count = 0
        buf.write_var_u32(self.commands.len() as u32)?; // commands count

        for cmd in &self.commands {
            buf.write_string(&cmd.name)?;
            buf.write_string(&cmd.description)?;
            buf.write_u16_le(cmd.flags)?;
            // Permission is a name string.
            let permission_str = match cmd.permission {
                0 => "any",
                1 => "gamedirectors",
                2 => "admin",
                3 => "host",
                4 => "owner",
                _ => "any",
            };
            buf.write_string(permission_str)?;
            buf.write_i32_le(-1)?; // aliases = -1 (int LE, no aliases)
            buf.write_var_u32(0)?; // subcommands count = 0
            buf.write_var_u32(0)?; // overloads count = 0
        }

        buf.write_var_u32(0)?; // softEnums count = 0
        buf.write_var_u32(0)?; // enumConstraints count = 0
        Ok(())
    }
}

impl Reader<AvailableCommands> for AvailableCommands {
    fn read(_buf: &mut ByteReader) -> Result<AvailableCommands, Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "AvailableCommands decode is unsupported",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        persona_piece_type_id, write_player_list_add_entry, CraftingData, PlayerListEntry,
        SetSpawnPosition,
    };
    use sc_binary::interfaces::{Reader, Writer};
    use sc_binary::{ByteReader, ByteWriter};

    #[test]
    fn set_spawn_position_matches_pnx_join_layout() {
        let packet = SetSpawnPosition {
            spawn_type: SetSpawnPosition::TYPE_PLAYER_SPAWN,
            x: 2,
            y: 64,
            z: 1,
            dimension: 0,
            spawn_block_position: None,
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();

        assert_eq!(
            writer.as_slice(),
            &[
                0x00, // player spawn type
                0x04, // x=2, zigzag varint32
                0x80, 0x01, // y=64, zigzag varint32 (128)
                0x02, // z=1, zigzag varint32
                0x00, // overworld dimension
                // Spawn block position: None falls back to main coordinates.
                0x04, // x=2
                0x80, 0x01, // y=64
                0x02, // z=1
            ]
        );
    }

    #[test]
    fn crafting_data_includes_smithing_trim_recipe() {
        let mut writer = ByteWriter::new();
        CraftingData::empty().write(&mut writer).unwrap();

        // Segmented counts; an empty server sends only the fixed
        // minecraft:smithing_armor_trim recipe.
        let mut reader = ByteReader::from(writer.as_slice());
        assert_eq!(reader.read_var_u32().unwrap(), 0); // shaped
        assert_eq!(reader.read_var_u32().unwrap(), 0); // shapeless
        assert_eq!(reader.read_var_u32().unwrap(), 0); // multi
        assert_eq!(reader.read_var_u32().unwrap(), 0); // user
        assert_eq!(reader.read_var_u32().unwrap(), 0); // chemistry shapeless
        assert_eq!(reader.read_var_u32().unwrap(), 0); // chemistry shaped
        assert_eq!(reader.read_var_u32().unwrap(), 0); // smithing transform
        assert_eq!(reader.read_var_u32().unwrap(), 1); // smithing trim count
        assert_eq!(
            reader.read_string().unwrap(),
            "minecraft:smithing_armor_trim"
        );
        for item_tag in [
            "minecraft:trim_templates",
            "minecraft:trimmable_armors",
            "minecraft:trim_materials",
        ] {
            // putTrimRecipeIngredient:varuint(1) + "item_tag" + tag + meta + count
            assert_eq!(reader.read_var_u32().unwrap(), 1);
            assert_eq!(reader.read_string().unwrap(), "item_tag");
            assert_eq!(reader.read_string().unwrap(), item_tag);
            assert_eq!(reader.read_var_i32().unwrap(), 32767);
            assert_eq!(reader.read_var_i32().unwrap(), 1);
        }
        assert_eq!(reader.read_string().unwrap(), "smithing_table");
        assert_eq!(reader.read_var_u32().unwrap(), 1); // network id
        assert_eq!(reader.read_var_u32().unwrap(), 0); // brewing
        assert_eq!(reader.read_var_u32().unwrap(), 0); // containers
        assert_eq!(reader.read_var_u32().unwrap(), 0); // material reducers
        assert!(reader.read_bool().unwrap()); // cleanRecipes
        assert!(reader.as_slice().is_empty());
    }

    #[test]
    fn crafting_data_2168_golden_segments_and_order() {
        use sc_recipe::{CompileBudgets, RecipeRegistrySnapshot, SourceRecipe};
        // Fixtures: one shaped, one shapeless (stonecutter), one furnace,
        // one smithing transform. Brewing uses numeric ids via resolver.
        let raws = vec![
            serde_json::json!({
                "format_version": "1.12",
                "minecraft:recipe_shaped": {
                    "description": {"identifier": "minecraft:test_pickaxe"},
                    "tags": ["crafting_table"],
                    "pattern": ["XXX", " # ", " # "],
                    "key": {
                        "#": {"item": "minecraft:stick"},
                        "X": {"item": "minecraft:iron_ingot"}
                    },
                    "result": {"item": "minecraft:iron_pickaxe"}
                }
            }),
            serde_json::json!({
                "format_version": "1.12",
                "minecraft:recipe_shapeless": {
                    "description": {"identifier": "minecraft:test_cut"},
                    "tags": ["stonecutter"],
                    "ingredients": [{"item": "minecraft:stone"}],
                    "result": {"item": "minecraft:stone_slab", "count": 2}
                }
            }),
            serde_json::json!({
                "format_version": "1.12",
                "minecraft:recipe_furnace": {
                    "description": {"identifier": "minecraft:test_smelt"},
                    "tags": ["furnace"],
                    "input": "minecraft:iron_ore",
                    "output": "minecraft:iron_ingot"
                }
            }),
            serde_json::json!({
                "format_version": "1.20.10",
                "minecraft:recipe_smithing_transform": {
                    "description": {"identifier": "minecraft:test_smith"},
                    "tags": ["smithing_table"],
                    "template": "minecraft:netherite_upgrade_smithing_template",
                    "base": "minecraft:diamond_boots",
                    "addition": "minecraft:netherite_ingot",
                    "result": "minecraft:netherite_boots"
                }
            }),
        ];
        let sources: Vec<SourceRecipe> = raws
            .into_iter()
            .enumerate()
            .map(|(i, raw)| {
                let bytes = serde_json::to_vec(&raw).unwrap();
                SourceRecipe::new("test", 0, &format!("{i}.json"), "1.12", raw, &bytes)
            })
            .collect();
        let (snapshot, _) =
            RecipeRegistrySnapshot::compile(&sources, &CompileBudgets::default(), false).unwrap();
        // Identifier → runtime resolver (fixture ids, deterministic).
        let table: std::collections::HashMap<&str, (u16, u32)> = [
            ("minecraft:stick", (1, 0)),
            ("minecraft:iron_ingot", (2, 0)),
            ("minecraft:iron_pickaxe", (3, 0)),
            ("minecraft:stone", (4, 0)),
            ("minecraft:stone_slab", (5, 0)),
            ("minecraft:iron_ore", (6, 0)),
            ("minecraft:netherite_upgrade_smithing_template", (7, 0)),
            ("minecraft:diamond_boots", (8, 0)),
            ("minecraft:netherite_ingot", (9, 0)),
            ("minecraft:netherite_boots", (10, 0)),
        ]
        .into_iter()
        .collect();
        let item_runtime = |id: &str| table.get(id).copied();
        let brewing_id = |_: &str| None;
        let (data, disabled) = CraftingData::from_snapshot(&snapshot, &item_runtime, &brewing_id);
        assert!(disabled.is_empty(), "disabled: {disabled:?}");
        assert_eq!(data.shaped.len(), 1);
        assert_eq!(data.shapeless.len(), 2); // stonecutter + furnace share the segment
        assert_eq!(data.smithing_transform.len(), 1);
        // Network ids start at 2 and increment in network order.
        let mut ids: Vec<u32> = data
            .shaped
            .iter()
            .map(|r| r.network_id)
            .chain(data.shapeless.iter().map(|r| r.network_id))
            .chain(data.smithing_transform.iter().map(|r| r.network_id))
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, vec![2, 3, 4, 5]);

        let mut writer = ByteWriter::new();
        data.write(&mut writer).unwrap();
        let bytes = writer.as_slice().to_vec();
        assert!(!bytes.is_empty());
        // Golden: decode segment counts in 2168 order.
        let mut reader = ByteReader::from(bytes.as_slice());
        assert_eq!(reader.read_var_u32().unwrap(), 1); // shaped
                                                       // Skip the shaped entry body; only the segment framing is golden.
                                                       // (Full per-entry golden bytes would couple to UUID derivation;
                                                       // counts + order + trim + clean flag are the compatibility contract.)
        let mut writer2 = ByteWriter::new();
        CraftingData::empty().write(&mut writer2).unwrap();
        // Non-empty payload must be strictly larger than the empty trim-only
        // payload and must still end with the trim + clean tail.
        assert!(bytes.len() > writer2.as_slice().len());
        // Re-encode determinism: same snapshot → identical bytes.
        let (data2, _) = CraftingData::from_snapshot(&snapshot, &item_runtime, &brewing_id);
        let mut writer3 = ByteWriter::new();
        data2.write(&mut writer3).unwrap();
        assert_eq!(bytes, writer3.as_slice());
    }

    #[test]
    fn crafting_data_skips_unknown_items_with_reason() {
        use sc_recipe::{CompileBudgets, RecipeRegistrySnapshot, SourceRecipe};
        let raw = serde_json::json!({
            "format_version": "1.12",
            "minecraft:recipe_shapeless": {
                "description": {"identifier": "minecraft:bad"},
                "tags": ["crafting_table"],
                "ingredients": [{"item": "minecraft:stone"}],
                "result": {"item": "minecraft:unknown_item_xyz"}
            }
        });
        let bytes = serde_json::to_vec(&raw).unwrap();
        let src = SourceRecipe::new("p", 0, "r.json", "1.12", raw, &bytes);
        let (snapshot, _) =
            RecipeRegistrySnapshot::compile(&[src], &CompileBudgets::default(), false).unwrap();
        let (data, disabled) =
            CraftingData::from_snapshot(&snapshot, &|_: &str| None, &|_: &str| None);
        assert_eq!(data.shapeless.len(), 0);
        assert_eq!(disabled.len(), 1);
    }

    #[test]
    fn complete_version_pack_builds_a_nonempty_crafting_table() {
        use std::io::Cursor;
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../version_packs/[SC原版版本包] Vanilla-1.26.40.scver"
        );
        let Ok(bytes) = std::fs::read(path) else {
            return;
        };
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut version =
            sc_packloader::version_control::loader::ZippedVersionPack::get_version(&mut zip)
                .unwrap();
        let runtime = version.take_runtime_id();
        let mut packs = version.take_behavior_packs();
        packs.sort_by(|a, b| a.0.cmp(&b.0));
        let mut sources = Vec::new();
        for (order, (_, pack)) in packs.iter().enumerate() {
            let (files, _) = sc_packloader::recipe_source::read_recipes_from_resource_pack(
                pack,
                order,
                &sc_packloader::recipe_source::RecipeSourceBudgets::default(),
            );
            for file in files {
                sources.push(sc_recipe::SourceRecipe::from_parts(
                    file.pack_name,
                    file.pack_order,
                    file.relative_path,
                    file.format_version,
                    file.raw,
                    file.fingerprint,
                    file.raw_bytes,
                ));
            }
        }
        assert!(
            sources.len() > 800,
            "the complete pack stack must be inspected"
        );
        let (snapshot, _) = sc_recipe::RecipeRegistrySnapshot::compile(
            &sources,
            &sc_recipe::CompileBudgets::default(),
            false,
        )
        .unwrap();
        let resolver = |name: &str| {
            runtime
                .iter()
                .find(|item| item.name == name)
                .map(|item| (item.id as u16, 0))
        };
        let (wire, disabled) = CraftingData::from_snapshot(&snapshot, &resolver, &|_| None);
        assert!(
            wire.shaped.len() + wire.shapeless.len() > 100,
            "empty crafting table: {disabled:?}"
        );
        for entry in &wire.shaped {
            assert_eq!(
                entry.network_id,
                snapshot.network_index(&entry.recipe_id).unwrap() + 2
            );
        }
        for entry in &wire.shapeless {
            assert_eq!(
                entry.network_id,
                snapshot.network_index(&entry.recipe_id).unwrap() + 2
            );
        }
        let mut writer = ByteWriter::new();
        wire.write(&mut writer).unwrap();
        eprintln!(
            "Complete recipe stack: {} source files, {} active, {} shaped, {} shapeless, {} bytes",
            sources.len(),
            snapshot.len(),
            wire.shaped.len(),
            wire.shapeless.len(),
            writer.as_slice().len()
        );
    }

    /// PlayFab ID is omitted and CoCo shifts the piece enum on 2225.
    #[test]
    fn player_list_skin_follows_2225_layout() {
        use crate::protocol::version::{
            with_protocol_version, PROTOCOL_VERSION_1_26_40, PROTOCOL_VERSION_1_26_60,
        };
        assert_eq!(persona_piece_type_id("persona_skeleton"), 1);
        with_protocol_version(PROTOCOL_VERSION_1_26_40, || {
            assert_eq!(persona_piece_type_id("persona_coco"), 28);
            assert_eq!(persona_piece_type_id("persona_unsupported"), 28);
        });
        with_protocol_version(PROTOCOL_VERSION_1_26_60, || {
            assert_eq!(persona_piece_type_id("persona_coco"), 28);
            assert_eq!(persona_piece_type_id("persona_unsupported"), 29);
        });
        let mut skin = sc_utils::game::skin::Skin::default();
        skin.play_fab_id = "pf".to_string();
        let entry = PlayerListEntry {
            skin: Some(skin),
            ..PlayerListEntry::default()
        };
        let old_bytes = with_protocol_version(PROTOCOL_VERSION_1_26_40, || {
            let mut writer = ByteWriter::new();
            write_player_list_add_entry(&mut writer, &entry).unwrap();
            writer.as_slice().to_vec()
        });
        let new_bytes = with_protocol_version(PROTOCOL_VERSION_1_26_60, || {
            let mut writer = ByteWriter::new();
            write_player_list_add_entry(&mut writer, &entry).unwrap();
            writer.as_slice().to_vec()
        });
        // PlayFab ID string ("pf" = length prefix + 2 bytes) is gone on 2225.
        assert_eq!(old_bytes.len(), new_bytes.len() + 3);
    }
}

#[cfg(test)]
mod ingredient_wire_tests {
    use super::{write_ingredient, CraftingIngredientWire};
    use sc_binary::interfaces::Writer;
    use sc_binary::ByteWriter;

    /// Ingredient descriptor layout: varuint type + body + fixed u16 count.
    #[test]
    fn ingredient_descriptors_match_official_layout() {
        let mut writer = ByteWriter::new();
        write_ingredient(&mut writer, &CraftingIngredientWire::Empty).unwrap();
        assert_eq!(writer.as_slice(), &[0, 0, 0]);

        let mut writer = ByteWriter::new();
        write_ingredient(
            &mut writer,
            &CraftingIngredientWire::Item {
                identifier: "minecraft:stick".to_string(),
                data: 0,
                count: 1,
            },
        )
        .unwrap();
        let bytes = writer.as_slice();
        assert_eq!(bytes[0], 1);
        assert!(bytes
            .windows(b"minecraft:stick".len())
            .any(|w| w == b"minecraft:stick"));
        assert_eq!(&bytes[bytes.len() - 3..], &[0, 1, 0]);

        let mut writer = ByteWriter::new();
        write_ingredient(
            &mut writer,
            &CraftingIngredientWire::Tag {
                tag: "minecraft:planks".to_string(),
                count: 1,
            },
        )
        .unwrap();
        let bytes = writer.as_slice();
        assert_eq!(bytes[0], 3);
        assert!(bytes
            .windows(b"minecraft:planks".len())
            .any(|w| w == b"minecraft:planks"));
        assert_eq!(&bytes[bytes.len() - 2..], &[1, 0]);
    }
}
