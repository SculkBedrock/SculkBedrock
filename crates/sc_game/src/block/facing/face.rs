//! 强类型几何面与空间轴。
//!
//! Strongly-typed block faces and spatial axes.

/// 空间三维轴：X（东西）、Y（上下）、Z（南北）。
///
/// 3D spatial axis: X (East/West), Y (Up/Down), Z (North/South).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    /// 转换为方块属性小写字符串 ("x", "y", "z")。
    ///
    /// Converts to the lowercase block-property string ("x", "y", "z").
    pub fn as_str(self) -> &'static str {
        match self {
            Self::X => "x",
            Self::Y => "y",
            Self::Z => "z",
        }
    }

    /// 该轴是否为水平轴（X 或 Z）。
    ///
    /// Whether this axis is horizontal (X or Z).
    pub fn is_horizontal(self) -> bool {
        matches!(self, Self::X | Self::Z)
    }

    /// 该轴是否为垂直轴（Y）。
    ///
    /// Whether this axis is vertical (Y).
    pub fn is_vertical(self) -> bool {
        matches!(self, Self::Y)
    }
}

/// 基岩版 6 向方块面：DOWN=0, UP=1, NORTH=2, SOUTH=3, WEST=4, EAST=5。
///
/// Bedrock 6-way block face ordinals matching protocol and face_offset.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum BlockFace {
    Down = 0,
    Up = 1,
    North = 2,
    South = 3,
    West = 4,
    East = 5,
}

impl BlockFace {
    /// 所有 6 个面的固定枚举数组。
    ///
    /// Fixed array of all 6 faces.
    pub const ALL: [BlockFace; 6] = [
        Self::Down,
        Self::Up,
        Self::North,
        Self::South,
        Self::West,
        Self::East,
    ];

    /// 所有 4 个水平面的固定枚举数组（南-西-北-东）。
    ///
    /// Fixed array of the 4 horizontal faces (south-west-north-east).
    pub const HORIZONTALS: [BlockFace; 4] = [
        Self::South,
        Self::West,
        Self::North,
        Self::East,
    ];

    /// 从原始序数（0..=5）转换。
    ///
    /// Converts from a raw ordinal (0..=5); returns `None` for out-of-range values.
    #[inline]
    pub fn from_index(index: u8) -> Option<Self> {
        match index {
            0 => Some(Self::Down),
            1 => Some(Self::Up),
            2 => Some(Self::North),
            3 => Some(Self::South),
            4 => Some(Self::West),
            5 => Some(Self::East),
            _ => None,
        }
    }

    /// 原始序数数值（0..=5）。
    ///
    /// The raw ordinal value (0..=5).
    #[inline]
    pub fn index(self) -> u8 {
        self as u8
    }

    /// 由身体偏航角（yaw，度）解析水平视线朝向（Yaw 四分区）。
    ///
    /// [315, 45) 为 SOUTH, [45, 135) 为 WEST, [135, 225) 为 NORTH, [225, 315) 为 EAST。
    ///
    /// Resolves the horizontal look face from body yaw in degrees (four yaw quadrants).
    ///
    /// SOUTH on [315, 45), WEST on [45, 135), NORTH on [135, 225), EAST on [225, 315);
    /// non-finite yaw returns SOUTH.
    pub fn from_yaw(yaw: f32) -> Self {
        if !yaw.is_finite() {
            return Self::South;
        }
        let rotation = (yaw % 360.0 + 360.0) % 360.0;
        if rotation < 45.0 || rotation >= 315.0 {
            Self::South
        } else if rotation < 135.0 {
            Self::West
        } else if rotation < 225.0 {
            Self::North
        } else {
            Self::East
        }
    }

    /// 反向面（熔炉正面朝向放置它的玩家，漏斗连接反面等）。
    ///
    /// The opposite face (furnace front faces the placing player, hopper connects the
    /// opposite side, etc.).
    #[inline]
    pub fn opposite(self) -> Self {
        match self {
            Self::Down => Self::Up,
            Self::Up => Self::Down,
            Self::North => Self::South,
            Self::South => Self::North,
            Self::West => Self::East,
            Self::East => Self::West,
        }
    }

    /// 该面所垂直对应的空间轴。
    ///
    /// The spatial axis this face is perpendicular to.
    #[inline]
    pub fn axis(self) -> Axis {
        match self {
            Self::Down | Self::Up => Axis::Y,
            Self::North | Self::South => Axis::Z,
            Self::West | Self::East => Axis::X,
        }
    }

    /// 是否为水平面（NORTH, SOUTH, WEST, EAST）。
    ///
    /// Whether this is a horizontal face (NORTH, SOUTH, WEST, EAST).
    #[inline]
    pub fn is_horizontal(self) -> bool {
        self.axis().is_horizontal()
    }

    /// 是否为垂直面（DOWN, UP）。
    ///
    /// Whether this is a vertical face (DOWN, UP).
    #[inline]
    pub fn is_vertical(self) -> bool {
        self.axis().is_vertical()
    }

    /// 转换为基岩版方位朝向名（`minecraft:cardinal_direction`）。
    ///
    /// Converts to the Bedrock cardinal-direction name (`minecraft:cardinal_direction`);
    /// non-cardinal faces fall back to "south".
    pub fn as_cardinal_str(self) -> &'static str {
        match self {
            Self::North => "north",
            Self::South => "south",
            Self::West => "west",
            Self::East => "east",
            _ => "south",
        }
    }

    /// 转换为侦测器式 6 向命名（`minecraft:facing_direction`）。
    ///
    /// Converts to the observer-style 6-way name (`minecraft:facing_direction`).
    pub fn as_observer_str(self) -> &'static str {
        match self {
            Self::Down => "down",
            Self::Up => "up",
            Self::North => "north",
            Self::South => "south",
            Self::West => "west",
            Self::East => "east",
        }
    }

    /// 转换为火把朝向名（`torch_facing_direction`）。
    ///
    /// 顶面为 `"top"`，侧面为面名；底面为 `None`（无法悬挂火把）。
    ///
    /// Converts to the torch-facing name (`torch_facing_direction`): top face is
    /// `"top"`, side faces use the face name, bottom returns `None` (no torch mount).
    pub fn as_torch_str(self) -> Option<&'static str> {
        match self {
            Self::Up => Some("top"),
            Self::North => Some("north"),
            Self::South => Some("south"),
            Self::West => Some("west"),
            Self::East => Some("east"),
            Self::Down => None,
        }
    }

    /// 转换为楼梯角度属性值（`weirdo_direction`：W=0, E=1, N=2, S=3）。
    ///
    /// Converts to the stairs angle value (`weirdo_direction`: W=0, E=1, N=2, S=3).
    pub fn as_weirdo_int(self) -> i32 {
        match self {
            Self::West => 0,
            Self::East => 1,
            Self::North => 2,
            Self::South => 3,
            _ => 2,
        }
    }

    /// 转换为活板门角度属性值（`direction`：EWSN 罗盘序 E=0, W=1, S=2, N=3）。
    ///
    /// Converts to the trapdoor direction value (`direction`: EWSN compass order
    /// E=0, W=1, S=2, N=3).
    pub fn as_trapdoor_int(self) -> i32 {
        match self {
            Self::East => 0,
            Self::West => 1,
            Self::South => 2,
            Self::North => 3,
            _ => 2,
        }
    }
}
