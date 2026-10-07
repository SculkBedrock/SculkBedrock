use crate::pack::ResourcePack;

pub mod pack;
pub mod zipped_loader;

pub trait PackLoaderTrait {
    fn get_resource_packs(&self) -> Vec<ResourcePack>;
}
