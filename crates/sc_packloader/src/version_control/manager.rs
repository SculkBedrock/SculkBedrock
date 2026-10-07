use crate::version_control::SCVersionPack;
use std::collections::HashMap;

pub struct VersionPackManager {
    pub map: HashMap<String, SCVersionPack>,
}
