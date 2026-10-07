//! Entity metadata constants and list helpers.
//!
//! - [`EntityKeys`]: metadata keys (e.g. DATA_AIR=7, DATA_MAX_AIR=42);
//! - [`EntityFlags`]: entity data flags (e.g. BREATHING=35,
//!   HAS_COLLISION=48, GRAVITY=49, SWIMMING=57);
//! - [`EntityMetadataExt`]: direct [`EntityMetadataEntry`] list access.

use super::misc::{EntityMetadataEntry, EntityMetadataValue};

/// Entity metadata keys.
pub mod EntityKeys {
    pub const FLAGS: u32 = 0; // long
    pub const HEALTH: u32 = 1; // int
    pub const VARIANT: u32 = 2; // int
    pub const COLOR: u32 = 3; // byte
    pub const NAMETAG: u32 = 4; // string
    pub const OWNER_EID: u32 = 5; // long
    pub const TARGET_EID: u32 = 6; // long
    pub const AIR: u32 = 7; // short (current air supply)
    pub const POTION_COLOR: u32 = 8; // int
    pub const POTION_AMBIENT: u32 = 9; // byte
    pub const JUMP_DURATION: u32 = 10; // long
    pub const HURT_TIME: u32 = 11; // int
    pub const HURT_DIRECTION: u32 = 12; // int
    pub const EXPERIENCE_VALUE: u32 = 15; // int
    pub const DISPLAY_ITEM: u32 = 16; // int
    pub const PLAYER_FLAGS: u32 = 26; // byte
    pub const PLAYER_INDEX: u32 = 27; // int
    pub const PLAYER_BED_POSITION: u32 = 28; // pos (Vector3i)
    pub const POTION_AUX_VALUE: u32 = 36; // short
    pub const LEAD_HOLDER_EID: u32 = 37; // long
    pub const SCALE: u32 = 38; // float
    pub const NPC_SKIN_ID: u32 = 40; // string
    pub const URL_TAG: u32 = 41; // string
    pub const MAX_AIR: u32 = 42; // short (maximum air supply)
    pub const BOUNDING_BOX_WIDTH: u32 = 53; // float
    pub const BOUNDING_BOX_HEIGHT: u32 = 54; // float
    pub const RIDER_SEAT_POSITION: u32 = 56; // vector3f
    pub const ALWAYS_SHOW_NAMETAG: u32 = 81; // byte
    pub const SCORE_TAG: u32 = 84; // string
    pub const FLAGS_EXTENDED: u32 = 92; // long (extended flags, 64 bits and up)
    pub const COLLISION_BOX: u32 = 130; // vector3f
    pub const VISIBLE_MOB_EFFECTS: u32 = 131; // long
    pub const FILTERED_NAME: u32 = 132; // string
    pub const BED_ENTER_POSITION: u32 = 133; // vector3f
}

/// Entity data flags.
pub mod EntityFlags {
    pub const ONFIRE: u8 = 0;
    pub const SNEAKING: u8 = 1;
    pub const RIDING: u8 = 2;
    pub const SPRINTING: u8 = 3;
    pub const ACTION: u8 = 4;
    pub const INVISIBLE: u8 = 5;
    pub const TEMPTED: u8 = 6;
    pub const INLOVE: u8 = 7;
    pub const SADDLED: u8 = 8;
    pub const POWERED: u8 = 9;
    pub const IGNITED: u8 = 10;
    pub const BABY: u8 = 11;
    pub const CONVERTING: u8 = 12;
    pub const CRITICAL: u8 = 13;
    pub const CAN_SHOW_NAMETAG: u8 = 14;
    pub const ALWAYS_SHOW_NAMETAG: u8 = 15;
    pub const IMMOBILE: u8 = 16;
    pub const SILENT: u8 = 17;
    pub const WALLCLIMBING: u8 = 18;
    pub const CAN_CLIMB: u8 = 19;
    pub const SWIMMER: u8 = 20;
    pub const CAN_FLY: u8 = 21;
    pub const WALKER: u8 = 22;
    pub const RESTING: u8 = 23;
    pub const SITTING: u8 = 24;
    pub const ANGRY: u8 = 25;
    pub const INTERESTED: u8 = 26;
    pub const CHARGED: u8 = 27;
    pub const TAMED: u8 = 28;
    pub const ORPHANED: u8 = 29;
    pub const LEASHED: u8 = 30;
    pub const SHEARED: u8 = 31;
    pub const GLIDING: u8 = 32;
    pub const ELDER: u8 = 33;
    pub const MOVING: u8 = 34;
    /// True on land (breathing), false in water.
    pub const BREATHING: u8 = 35;
    pub const CHESTED: u8 = 36;
    pub const STACKABLE: u8 = 37;
    pub const SHOWBASE: u8 = 38;
    pub const REARING: u8 = 39;
    pub const VIBRATING: u8 = 40;
    pub const IDLING: u8 = 41;
    pub const EVOKER_SPELL: u8 = 42;
    pub const CHARGE_ATTACK: u8 = 43;
    pub const WASD_CONTROLLED: u8 = 44;
    pub const CAN_POWER_JUMP: u8 = 45;
    pub const CAN_DASH: u8 = 46;
    pub const LINGER: u8 = 47;
    pub const HAS_COLLISION: u8 = 48;
    pub const GRAVITY: u8 = 49;
    pub const FIRE_IMMUNE: u8 = 50;
    pub const DANCING: u8 = 51;
    pub const ENCHANTED: u8 = 52;
    pub const SHOW_TRIDENT_ROPE: u8 = 53;
    pub const CONTAINER_PRIVATE: u8 = 54;
    pub const IS_TRANSFORMING: u8 = 55;
    pub const SPIN_ATTACK: u8 = 56;
    pub const SWIMMING: u8 = 57;
    pub const BRIBED: u8 = 58;
    pub const PREGNANT: u8 = 59;
    pub const LAYING_EGG: u8 = 60;
    pub const RIDER_CAN_PICK: u8 = 61;
    pub const TRANSITION_SETTING: u8 = 62;
    pub const EATING: u8 = 63;
    pub const LAYING_DOWN: u8 = 64;
    pub const SNEEZING: u8 = 65;
    pub const TRUSTING: u8 = 66;
    pub const ROLLING: u8 = 67;
    pub const SCARED: u8 = 68;
    pub const IN_SCAFFOLDING: u8 = 69;
    pub const OVER_SCAFFOLDING: u8 = 70;
    pub const FALL_THROUGH_SCAFFOLDING: u8 = 71;
    pub const BLOCKING: u8 = 72;
    pub const TRANSITION_BLOCKING: u8 = 73;
    pub const BLOCKED_USING_SHIELD: u8 = 74;
    pub const BLOCKED_USING_DAMAGED_SHIELD: u8 = 75;
    pub const SLEEPING: u8 = 76;
    pub const ENTITY_GROW_UP: u8 = 77;
    pub const TRADE_INTEREST: u8 = 78;
    pub const DOOR_BREAKER: u8 = 79;
    pub const BREAKING_OBSTRUCTION: u8 = 80;
    pub const DOOR_OPENER: u8 = 81;
    pub const IS_ILLAGER_CAPTAIN: u8 = 82;
    pub const STUNNED: u8 = 83;
    pub const ROARING: u8 = 84;
    pub const DELAYED_ATTACK: u8 = 85;
    pub const IS_AVOIDING_MOBS: u8 = 86;
    pub const IS_AVOIDING_BLOCKS: u8 = 87;
    pub const FACING_TARGET_TO_RANGE_ATTACK: u8 = 88;
    pub const HIDDEN_WHEN_INVISIBLE: u8 = 89;
    pub const IS_IN_UI: u8 = 90;
    pub const STALKING: u8 = 91;
    pub const EMOTING: u8 = 92;
    pub const CELEBRATING: u8 = 93;
    pub const ADMIRING: u8 = 94;
    pub const CELEBRATING_SPECIAL: u8 = 95;
    pub const OUT_OF_CONTROL: u8 = 96;
    pub const RAM_ATTACK: u8 = 97;
    pub const PLAYING_DEAD: u8 = 98;
    pub const IN_ASCENDABLE_BLOCK: u8 = 99;
    pub const OVER_DESCENDABLE_BLOCK: u8 = 100;
    pub const CROAKING: u8 = 101;
    pub const EAT_MOB: u8 = 102;
    pub const JUMP_GOAL_JUMP: u8 = 103;
    pub const EMERGING: u8 = 104;
    pub const SNIFFING: u8 = 105;
    pub const DIGGING: u8 = 106;
    pub const SONIC_BOOM: u8 = 107;
    pub const HAS_DASH_COOLDOWN: u8 = 108;
    pub const PUSH_TOWARDS_CLOSEST_SPACE: u8 = 109;
    pub const SCENTING: u8 = 110;
    pub const RISING: u8 = 111;
    pub const FEELING_HAPPY: u8 = 112;
    pub const SEARCHING: u8 = 113;
    pub const CRAWLING: u8 = 114;
    pub const TIMER_FLAG_1: u8 = 115;
    pub const TIMER_FLAG_2: u8 = 116;
    pub const TIMER_FLAG_3: u8 = 117;
    pub const BODY_ROTATION_BLOCKED: u8 = 118;
    pub const RENDER_WHEN_INVISIBLE: u8 = 119;
    pub const BODY_ROTATION_AXIS_ALIGNED: u8 = 120;
    pub const COLLIDABLE: u8 = 121;
    pub const WASD_AIR_CONTROLLED: u8 = 122;
    pub const DOES_SERVER_AUTH_ONLY_DISMOUNT: u8 = 123;
    pub const BODY_ROTATION_ALWAYS_FOLLOWS_HEAD: u8 = 124;
    pub const CAN_USE_VERTICAL_MOVEMENT_ACTION: u8 = 125;
    pub const BODY_ROTATION_LOCKED_TO_VEHICLE: u8 = 126;
    pub const USES_LEGACY_FRICTION: u8 = 127;
    pub const USES_UNIFORM_AIR_DRAG: u8 = 128;
    pub const NAMEPLATE_DEPTH_TESTED: u8 = 129;
}

/// Metadata value type ids.
pub mod EntityDataTypes {
    pub const BYTE: u32 = 0;
    pub const SHORT: u32 = 1;
    pub const INT: u32 = 2;
    pub const FLOAT: u32 = 3;
    pub const STRING: u32 = 4;
    pub const NBT: u32 = 5;
    pub const POS: u32 = 6;
    pub const LONG: u32 = 7;
    pub const VECTOR3F: u32 = 8;
}

/// Direct [`EntityMetadataEntry`] list access (update in place when the
/// key exists, append otherwise).
pub trait EntityMetadataExt {
    fn set_flag(&mut self, flag: u8, value: bool);
    fn get_flag(&self, flag: u8) -> bool;
    fn set_byte(&mut self, key: u32, value: i8);
    fn set_short(&mut self, key: u32, value: i16);
    fn set_int(&mut self, key: u32, value: i32);
    fn set_float(&mut self, key: u32, value: f32);
    fn set_string(&mut self, key: u32, value: String);
    fn set_long(&mut self, key: u32, value: i64);
    fn set_vector3i(&mut self, key: u32, x: i32, y: i32, z: i32);
    fn set_vector3f(&mut self, key: u32, x: f32, y: f32, z: f32);
}

impl EntityMetadataExt for Vec<EntityMetadataEntry> {
    fn set_flag(&mut self, flag: u8, value: bool) {
        // FLAGS (key 0) entries update bitwise; missing entries start as Long(0).
        let flags_key = EntityKeys::FLAGS;
        let Some(entry) = self.iter_mut().find(|e| e.key == flags_key) else {
            let mut flags = 0i64;
            if value {
                flags |= 1i64 << flag;
            }
            self.push(EntityMetadataEntry {
                key: flags_key,
                data_type: EntityDataTypes::LONG,
                value: EntityMetadataValue::Long(flags),
            });
            return;
        };
        if let EntityMetadataValue::Long(current) = &mut entry.value {
            if value {
                *current |= 1i64 << flag;
            } else {
                *current &= !(1i64 << flag);
            }
        }
    }

    fn get_flag(&self, flag: u8) -> bool {
        self.iter()
            .find(|e| e.key == EntityKeys::FLAGS)
            .and_then(|e| match &e.value {
                EntityMetadataValue::Long(v) => Some(v & (1i64 << flag) != 0),
                _ => None,
            })
            .unwrap_or(false)
    }

    fn set_byte(&mut self, key: u32, value: i8) {
        upsert(
            self,
            key,
            EntityDataTypes::BYTE,
            EntityMetadataValue::Byte(value),
        );
    }

    fn set_short(&mut self, key: u32, value: i16) {
        upsert(
            self,
            key,
            EntityDataTypes::SHORT,
            EntityMetadataValue::Short(value),
        );
    }

    fn set_int(&mut self, key: u32, value: i32) {
        upsert(
            self,
            key,
            EntityDataTypes::INT,
            EntityMetadataValue::Int(value),
        );
    }

    fn set_float(&mut self, key: u32, value: f32) {
        upsert(
            self,
            key,
            EntityDataTypes::FLOAT,
            EntityMetadataValue::Float(value),
        );
    }

    fn set_string(&mut self, key: u32, value: String) {
        upsert(
            self,
            key,
            EntityDataTypes::STRING,
            EntityMetadataValue::String(value),
        );
    }

    fn set_long(&mut self, key: u32, value: i64) {
        upsert(
            self,
            key,
            EntityDataTypes::LONG,
            EntityMetadataValue::Long(value),
        );
    }

    fn set_vector3i(&mut self, key: u32, x: i32, y: i32, z: i32) {
        upsert(
            self,
            key,
            EntityDataTypes::POS,
            EntityMetadataValue::Vector3i(x, y, z),
        );
    }

    fn set_vector3f(&mut self, key: u32, x: f32, y: f32, z: f32) {
        upsert(
            self,
            key,
            EntityDataTypes::VECTOR3F,
            EntityMetadataValue::Vector3f(x, y, z),
        );
    }
}

/// Update or insert a metadata entry (same key replaces, otherwise appends).
fn upsert(
    entries: &mut Vec<EntityMetadataEntry>,
    key: u32,
    data_type: u32,
    value: EntityMetadataValue,
) {
    if let Some(entry) = entries.iter_mut().find(|e| e.key == key) {
        entry.data_type = data_type;
        entry.value = value;
    } else {
        entries.push(EntityMetadataEntry {
            key,
            data_type,
            value,
        });
    }
}
