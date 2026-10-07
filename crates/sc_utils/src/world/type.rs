#[derive(Debug, Clone, Ord, PartialOrd, Eq, PartialEq)]
pub enum WorldType {
    Custom(Box<WorldType>),
    Overworld,
    TheNether,
    TheEnd,
}

impl WorldType {
    pub fn get_dimension(&self) -> i32 {
        match self {
            WorldType::Custom(ty) => ty.get_dimension(),
            WorldType::Overworld => 0,
            WorldType::TheNether => 1,
            WorldType::TheEnd => 2,
        }
    }
}
