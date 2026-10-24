use crate::pack::manifest::ResourcePackInformation;

/// 子包类型：资源包或行为包。
///
/// Sub-pack type: resource pack or behavior pack.
#[derive(Clone, Debug)]
pub enum SubPackType {
    ResourcePack,
    BehaviorPack,
}

/// 子包脚本语言。
///
/// Sub-pack script language.
#[derive(Clone, Debug)]
pub enum SubPackLanguage {
    TypeScript,
    JavaScript,
}

/// 子包信息：清单 + 类型 + 可选脚本语言。
///
/// Sub-pack information: manifest plus type plus optional script language.
#[derive(Clone, Debug)]
pub struct SubPackInformation {
    /// 子包清单信息。
    ///
    /// Sub-pack manifest information.
    pub information: ResourcePackInformation,
    /// 子包类型。
    ///
    /// Sub-pack type.
    pub pack_type: SubPackType,
    /// 可选的脚本语言。
    ///
    /// Optional script language.
    pub language: Option<SubPackLanguage>,
}
