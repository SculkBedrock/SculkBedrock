//! 版本包管理器：按名称索引已加载的版本包。
//!
//! Version pack manager: indexes loaded version packs by name.

use crate::version_control::SCVersionPack;
use std::collections::HashMap;

/// 版本包管理器：按名称索引已加载的版本包。
///
/// Version pack manager: indexes loaded version packs by name.
pub struct VersionPackManager {
    /// 名称到版本包的映射。
    ///
    /// Map from pack name to version pack.
    pub map: HashMap<String, SCVersionPack>,
}
