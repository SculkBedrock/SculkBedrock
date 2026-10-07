use std::collections::HashSet;
use std::io::{Error, ErrorKind};

use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;
use sc_utils::game::structs::position::MinecraftPosition;

/// PlayerAuthInputPacket, protocol 2168.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct PlayerAuthInput {
    pub item_stack_request: Option<super::crafting_request::ItemStackRequestEntry>,
    pub pitch: f32,
    pub yaw: f32,
    pub position: MinecraftPosition,
    pub move_vector_x: f32,
    pub move_vector_z: f32,
    pub head_yaw: f32,
    pub input_data: HashSet<AuthInputAction>,
    pub block_actions: Vec<PlayerBlockAction>,
    pub input_mode: PlayerInputMode,
    pub play_mode: u32,
    pub interaction_model: i32,
    pub interact_rotation_x: f32,
    pub interact_rotation_z: f32,
    pub tick: u64,
    pub delta: MinecraftPosition,
    pub analog_move_x: f32,
    pub analog_move_y: f32,
    pub camera_orientation_x: f32,
    pub camera_orientation_y: f32,
    pub camera_orientation_z: f32,
    pub raw_move_x: f32,
    pub raw_move_y: f32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerBlockAction {
    pub action_type: i32,
    pub block_x: i32,
    pub block_y: i32,
    pub block_z: i32,
    pub face: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum AuthInputAction {
    Ascend = 0,
    Descend = 1,
    NorthJump = 2,
    JumpDown = 3,
    SprintDown = 4,
    ChangeHeight = 5,
    Jumping = 6,
    AutoJumpingInWater = 7,
    Sneaking = 8,
    SneakDown = 9,
    Up = 10,
    Down = 11,
    Left = 12,
    Right = 13,
    UpLeft = 14,
    UpRight = 15,
    WantUp = 16,
    WantDown = 17,
    WantDownSlow = 18,
    WantUpSlow = 19,
    Sprinting = 20,
    AscendScaffolding = 21,
    DescendScaffolding = 22,
    SneakToggleDown = 23,
    PersistSneak = 24,
    StartSprinting = 25,
    StopSprinting = 26,
    StartSneaking = 27,
    StopSneaking = 28,
    StartSwimming = 29,
    StopSwimming = 30,
    StartJumping = 31,
    StartGliding = 32,
    StopGliding = 33,
    PerformItemInteraction = 34,
    PerformBlockActions = 35,
    PerformItemStackRequest = 36,
    HandleTeleport = 37,
    Emoting = 38,
    MissedSwing = 39,
    StartCrawling = 40,
    StopCrawling = 41,
    StartFlying = 42,
    StopFlying = 43,
    ReceivedServerData = 44,
    InClientPredictedInVehicle = 45,
    PaddleLeft = 46,
    PaddleRight = 47,
    BlockBreakingDelayEnabled = 48,
    HorizontalCollision = 49,
    VerticalCollision = 50,
    DownLeft = 51,
    DownRight = 52,
    StartUsingItem = 53,
    IsCameraRelativeMovementEnabled = 54,
    IsRotControlledByMoveDirection = 55,
    StartSpinAttack = 56,
    StopSpinAttack = 57,
    HotbarOnlyTouch = 58,
    JumpReleasedRaw = 59,
    JumpPressedRaw = 60,
    JumpCurrentRaw = 61,
    SneakReleasedRaw = 62,
    SneakPressedRaw = 63,
    SneakCurrentRaw = 64,
    InternalUpdate = 65,
}

impl AuthInputAction {
    pub const ALL: [AuthInputAction; 66] = [
        Self::Ascend,
        Self::Descend,
        Self::NorthJump,
        Self::JumpDown,
        Self::SprintDown,
        Self::ChangeHeight,
        Self::Jumping,
        Self::AutoJumpingInWater,
        Self::Sneaking,
        Self::SneakDown,
        Self::Up,
        Self::Down,
        Self::Left,
        Self::Right,
        Self::UpLeft,
        Self::UpRight,
        Self::WantUp,
        Self::WantDown,
        Self::WantDownSlow,
        Self::WantUpSlow,
        Self::Sprinting,
        Self::AscendScaffolding,
        Self::DescendScaffolding,
        Self::SneakToggleDown,
        Self::PersistSneak,
        Self::StartSprinting,
        Self::StopSprinting,
        Self::StartSneaking,
        Self::StopSneaking,
        Self::StartSwimming,
        Self::StopSwimming,
        Self::StartJumping,
        Self::StartGliding,
        Self::StopGliding,
        Self::PerformItemInteraction,
        Self::PerformBlockActions,
        Self::PerformItemStackRequest,
        Self::HandleTeleport,
        Self::Emoting,
        Self::MissedSwing,
        Self::StartCrawling,
        Self::StopCrawling,
        Self::StartFlying,
        Self::StopFlying,
        Self::ReceivedServerData,
        Self::InClientPredictedInVehicle,
        Self::PaddleLeft,
        Self::PaddleRight,
        Self::BlockBreakingDelayEnabled,
        Self::HorizontalCollision,
        Self::VerticalCollision,
        Self::DownLeft,
        Self::DownRight,
        Self::StartUsingItem,
        Self::IsCameraRelativeMovementEnabled,
        Self::IsRotControlledByMoveDirection,
        Self::StartSpinAttack,
        Self::StopSpinAttack,
        Self::HotbarOnlyTouch,
        Self::JumpReleasedRaw,
        Self::JumpPressedRaw,
        Self::JumpCurrentRaw,
        Self::SneakReleasedRaw,
        Self::SneakPressedRaw,
        Self::SneakCurrentRaw,
        Self::InternalUpdate,
    ];

    fn from_ordinal(ordinal: i32) -> Option<Self> {
        (ordinal >= 0)
            .then_some(ordinal as usize)
            .and_then(|index| Self::ALL.get(index).copied())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PlayerInputMode {
    Unknown = 0,
    Mouse = 1,
    Touch = 2,
    GamePad = 3,
    MotionController = 4,
}

impl PlayerInputMode {
    fn from_ordinal(ordinal: u32) -> Self {
        match ordinal {
            1 => Self::Mouse,
            2 => Self::Touch,
            3 => Self::GamePad,
            4 => Self::MotionController,
            _ => Self::Unknown,
        }
    }
}

impl PlayerAuthInput {
    pub const EYE_HEIGHT: f32 = 1.62;

    pub fn feet_position(&self) -> MinecraftPosition {
        MinecraftPosition::new(
            self.position.x,
            self.position.y - Self::EYE_HEIGHT,
            self.position.z,
        )
    }

    pub fn has_input(&self, action: AuthInputAction) -> bool {
        self.input_data.contains(&action)
    }

    pub fn sneaking(&self) -> bool {
        self.has_input(AuthInputAction::Sneaking) || self.has_input(AuthInputAction::SneakDown)
    }

    pub fn jumping(&self) -> bool {
        self.has_input(AuthInputAction::Jumping) || self.has_input(AuthInputAction::JumpDown)
    }

    pub fn sprinting(&self) -> bool {
        self.has_input(AuthInputAction::Sprinting)
            || self.has_input(AuthInputAction::StartSprinting)
    }

    pub fn flying(&self) -> bool {
        self.has_input(AuthInputAction::StartFlying)
    }
}

fn read_optional_legacy_pair(buf: &mut ByteReader) -> Result<bool, Error> {
    if !buf.read_bool()? {
        return Ok(false);
    }
    Ok(buf.read_bool()?)
}

fn read_block_actions(buf: &mut ByteReader) -> Result<Vec<PlayerBlockAction>, Error> {
    let count = buf.read_var_u32()? as usize;
    if count > 100 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "too many player block actions",
        ));
    }

    let mut actions = Vec::with_capacity(count);
    for _ in 0..count {
        actions.push(PlayerBlockAction {
            action_type: buf.read_var_i32()?,
            block_x: buf.read_var_i32()?,
            block_y: buf.read_var_i32()?,
            block_z: buf.read_var_i32()?,
            face: buf.read_var_i32()?,
        });
    }
    Ok(actions)
}

fn write_block_actions(buf: &mut ByteWriter, actions: &[PlayerBlockAction]) -> Result<(), Error> {
    if actions.is_empty() {
        buf.write_bool(false)?;
        return Ok(());
    }

    buf.write_bool(true)?;
    buf.write_bool(true)?;
    buf.write_var_u32(actions.len() as u32)?;
    for action in actions {
        buf.write_var_i32(action.action_type)?;
        buf.write_var_i32(action.block_x)?;
        buf.write_var_i32(action.block_y)?;
        buf.write_var_i32(action.block_z)?;
        buf.write_var_i32(action.face)?;
    }
    Ok(())
}

fn write_input_data(buf: &mut ByteWriter, actions: &HashSet<AuthInputAction>) -> Result<(), Error> {
    buf.write_bool(!actions.is_empty())?;
    if actions.is_empty() {
        return Ok(());
    }
    let count = AuthInputAction::ALL
        .iter()
        .filter(|action| actions.contains(action))
        .count();
    buf.write_var_u32(count as u32)?;
    for action in AuthInputAction::ALL
        .iter()
        .filter(|action| actions.contains(action))
    {
        buf.write_var_i32(*action as i32)?;
    }
    Ok(())
}

impl Reader<PlayerAuthInput> for PlayerAuthInput {
    fn read(buf: &mut ByteReader) -> Result<PlayerAuthInput, Error> {
        let pitch = buf.read_f32_le()?;
        let yaw = buf.read_f32_le()?;
        let x = buf.read_f32_le()?;
        let y = buf.read_f32_le()?;
        let z = buf.read_f32_le()?;
        let move_vector_x = buf.read_f32_le()?;
        let move_vector_z = buf.read_f32_le()?;
        let head_yaw = buf.read_f32_le()?;

        let mut input_data = HashSet::new();
        if buf.read_bool()? {
            let count = buf.read_var_u32()? as usize;
            if count > 100 {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "too many auth input actions",
                ));
            }
            for _ in 0..count {
                let ordinal = buf.read_var_i32()?;
                let action = AuthInputAction::from_ordinal(ordinal).ok_or_else(|| {
                    Error::new(ErrorKind::InvalidData, "unknown auth input action")
                })?;
                input_data.insert(action);
            }
        }

        let input_mode = PlayerInputMode::from_ordinal(buf.read_var_u32()?);
        let play_mode = buf.read_var_u32()?;
        let interaction_model = buf.read_var_i32()?;
        let interact_rotation_x = buf.read_f32_le()?;
        let interact_rotation_z = buf.read_f32_le()?;
        let tick = buf.read_var_u64()?;
        let delta =
            MinecraftPosition::new(buf.read_f32_le()?, buf.read_f32_le()?, buf.read_f32_le()?);

        if read_optional_legacy_pair(buf)? {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "item interaction is unsupported",
            ));
        }
        let item_stack_request = if read_optional_legacy_pair(buf)? {
            Some(super::crafting_request::ItemStackRequestEntry::read_entry(
                buf,
            )?)
        } else {
            None
        };
        let block_actions = if read_optional_legacy_pair(buf)? {
            read_block_actions(buf)?
        } else {
            Vec::new()
        };

        let (vehicle_rotation_x, vehicle_rotation_z) = if read_optional_legacy_pair(buf)? {
            (buf.read_f32_le()?, buf.read_f32_le()?)
        } else {
            (0.0, 0.0)
        };
        let predicted_vehicle = if read_optional_legacy_pair(buf)? {
            let _ = buf.read_var_i64()?;
            true
        } else {
            false
        };
        let _ = (vehicle_rotation_x, vehicle_rotation_z, predicted_vehicle);

        let analog_move_x = buf.read_f32_le()?;
        let analog_move_y = buf.read_f32_le()?;
        let camera_orientation_x = buf.read_f32_le()?;
        let camera_orientation_y = buf.read_f32_le()?;
        let camera_orientation_z = buf.read_f32_le()?;
        let raw_move_x = buf.read_f32_le()?;
        let raw_move_y = buf.read_f32_le()?;

        Ok(Self {
            item_stack_request,
            pitch,
            yaw,
            position: MinecraftPosition::new(x, y, z),
            move_vector_x,
            move_vector_z,
            head_yaw,
            input_data,
            block_actions,
            input_mode,
            play_mode,
            interaction_model,
            interact_rotation_x,
            interact_rotation_z,
            tick,
            delta,
            analog_move_x,
            analog_move_y,
            camera_orientation_x,
            camera_orientation_y,
            camera_orientation_z,
            raw_move_x,
            raw_move_y,
        })
    }
}

impl Writer for PlayerAuthInput {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_f32_le(self.pitch)?;
        buf.write_f32_le(self.yaw)?;
        buf.write_f32_le(self.position.x)?;
        buf.write_f32_le(self.position.y)?;
        buf.write_f32_le(self.position.z)?;
        buf.write_f32_le(self.move_vector_x)?;
        buf.write_f32_le(self.move_vector_z)?;
        buf.write_f32_le(self.head_yaw)?;
        let mut input_data = self.input_data.clone();
        if !self.block_actions.is_empty() {
            input_data.insert(AuthInputAction::PerformBlockActions);
        }
        if self.item_stack_request.is_some() {
            input_data.insert(AuthInputAction::PerformItemStackRequest);
        }
        write_input_data(buf, &input_data)?;
        buf.write_var_u32(self.input_mode as u32)?;
        buf.write_var_u32(self.play_mode)?;
        buf.write_var_i32(self.interaction_model)?;
        buf.write_f32_le(self.interact_rotation_x)?;
        buf.write_f32_le(self.interact_rotation_z)?;
        buf.write_var_u64(self.tick)?;
        buf.write_f32_le(self.delta.x)?;
        buf.write_f32_le(self.delta.y)?;
        buf.write_f32_le(self.delta.z)?;

        buf.write_bool(false)?;
        if let Some(request) = &self.item_stack_request {
            buf.write_bool(true)?;
            buf.write_bool(true)?;
            let mut encoded = ByteWriter::new();
            super::crafting_request::ItemStackRequest {
                requests: vec![request.clone()],
            }
            .write(&mut encoded)?;
            buf.write(&encoded.as_slice()[1..])?;
        } else {
            buf.write_bool(false)?;
        }
        write_block_actions(buf, &self.block_actions)?;
        buf.write_bool(false)?;
        buf.write_bool(false)?;
        buf.write_f32_le(self.analog_move_x)?;
        buf.write_f32_le(self.analog_move_y)?;
        buf.write_f32_le(self.camera_orientation_x)?;
        buf.write_f32_le(self.camera_orientation_y)?;
        buf.write_f32_le(self.camera_orientation_z)?;
        buf.write_f32_le(self.raw_move_x)?;
        buf.write_f32_le(self.raw_move_y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_auth_input_round_trips_block_actions() {
        let mut input_data = HashSet::new();
        input_data.insert(AuthInputAction::PerformBlockActions);
        let mut packet = PlayerAuthInput {
            item_stack_request: None,
            pitch: 10.0,
            yaw: 20.0,
            position: MinecraftPosition::new(1.0, 65.0, -2.0),
            move_vector_x: 0.5,
            move_vector_z: -0.25,
            head_yaw: 30.0,
            input_data,
            block_actions: vec![
                PlayerBlockAction {
                    action_type: 0,
                    block_x: -3,
                    block_y: 64,
                    block_z: 7,
                    face: 1,
                },
                PlayerBlockAction {
                    action_type: 26,
                    block_x: 4,
                    block_y: -12,
                    block_z: -8,
                    face: 5,
                },
            ],
            input_mode: PlayerInputMode::Mouse,
            play_mode: 0,
            interaction_model: 0,
            interact_rotation_x: 0.0,
            interact_rotation_z: 0.0,
            tick: 42,
            delta: MinecraftPosition::new(0.0, 0.0, 0.0),
            analog_move_x: 0.0,
            analog_move_y: 0.0,
            camera_orientation_x: 0.0,
            camera_orientation_y: 0.0,
            camera_orientation_z: 0.0,
            raw_move_x: 0.0,
            raw_move_y: 0.0,
        };

        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        let decoded = PlayerAuthInput::read(&mut ByteReader::from(writer)).unwrap();

        assert_eq!(decoded.block_actions, packet.block_actions);
        assert!(decoded.has_input(AuthInputAction::PerformBlockActions));
        packet.item_stack_request = Some(super::super::crafting_request::ItemStackRequestEntry {
            request_id: -7,
            actions: vec![
                super::super::crafting_request::ItemStackRequestAction::CraftRecipe {
                    recipe_network_id: 2,
                    times: 3,
                },
            ],
        });
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        let decoded = PlayerAuthInput::read(&mut ByteReader::from(writer)).unwrap();
        assert_eq!(decoded.item_stack_request, packet.item_stack_request);
        assert_eq!(decoded.block_actions, packet.block_actions);
        assert_eq!(decoded.tick, 42);
        assert_eq!(decoded.raw_move_y, packet.raw_move_y);
    }

    #[test]
    fn player_auth_input_without_block_actions_keeps_optional_fields_aligned() {
        let packet = PlayerAuthInput {
            item_stack_request: None,
            pitch: 0.0,
            yaw: 0.0,
            position: MinecraftPosition::new(0.0, 64.0, 0.0),
            move_vector_x: 0.0,
            move_vector_z: 0.0,
            head_yaw: 0.0,
            input_data: HashSet::new(),
            block_actions: Vec::new(),
            input_mode: PlayerInputMode::Unknown,
            play_mode: 0,
            interaction_model: 0,
            interact_rotation_x: 0.0,
            interact_rotation_z: 0.0,
            tick: 1,
            delta: MinecraftPosition::new(0.0, 0.0, 0.0),
            analog_move_x: 0.0,
            analog_move_y: 0.0,
            camera_orientation_x: 0.0,
            camera_orientation_y: 0.0,
            camera_orientation_z: 0.0,
            raw_move_x: 0.0,
            raw_move_y: 0.0,
        };

        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        let decoded = PlayerAuthInput::read(&mut ByteReader::from(writer)).unwrap();

        assert!(decoded.block_actions.is_empty());
        assert_eq!(decoded.raw_move_y, 0.0);
    }
}
