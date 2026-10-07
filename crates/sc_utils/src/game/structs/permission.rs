#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerPermission {
    Visitor,
    Member,
    Operator,
    Custom,
}

impl PlayerPermission {
    pub fn from_string(permission: &str) -> Option<Self> {
        match permission {
            "VISITOR" => Some(Self::Visitor),
            "MEMBER" => Some(Self::Member),
            "OPERATOR" => Some(Self::Operator),
            "CUSTOM" => Some(Self::Custom),
            _ => None,
        }
    }

    pub fn index(&self) -> usize {
        match self {
            Self::Visitor => 0,
            Self::Member => 1,
            Self::Operator => 2,
            Self::Custom => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandPermission {
    Normal,
    Operator,
    Host,
    Automation,
    Admin,
}

impl CommandPermission {
    pub fn from_string(permission: &str) -> Option<Self> {
        match permission {
            "NORMAL" => Some(Self::Normal),
            "OPERATOR" => Some(Self::Operator),
            "HOST" => Some(Self::Host),
            "AUTOMATION" => Some(Self::Automation),
            "ADMIN" => Some(Self::Admin),
            _ => None,
        }
    }

    pub fn index(&self) -> usize {
        match self {
            Self::Normal => 0,
            Self::Operator => 1,
            Self::Host => 2,
            Self::Automation => 3,
            Self::Admin => 4,
        }
    }
}
