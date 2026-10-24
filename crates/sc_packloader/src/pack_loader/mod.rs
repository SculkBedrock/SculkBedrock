//! 资源包加载器：从目录或压缩包中发现并读取资源包。
//!
//! Resource pack loaders: discover and read packs from directories or archives.

use crate::pack::ResourcePack;

pub mod pack;
pub mod zipped_loader;

/// 资源包加载器统一接口。
///
/// Common trait for resource pack loaders.
pub trait PackLoaderTrait {
    /// 加载并返回全部可解析的资源包。
    ///
    /// Loads and returns all parseable resource packs.
    fn get_resource_packs(&self) -> Vec<ResourcePack>;
}
