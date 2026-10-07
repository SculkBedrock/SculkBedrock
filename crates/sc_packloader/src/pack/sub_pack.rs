use crate::pack::manifest::ResourcePackInformation;

#[derive(Clone, Debug)]
pub enum SubPackType {
    ResourcePack,
    BehaviorPack,
}

#[derive(Clone, Debug)]
pub enum SubPackLanguage {
    TypeScript,
    JavaScript,
}

#[derive(Clone, Debug)]
pub struct SubPackInformation {
    pub information: ResourcePackInformation,
    pub pack_type: SubPackType,
    pub language: Option<SubPackLanguage>,
}
