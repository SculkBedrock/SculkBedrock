//! 创造模式发放表：创造列表 id 到可发放物品堆模板。
//!
//! Creative-mode grant table: creative listing ids to grantable stack templates.

use crate::stack::ItemStack;
use sc_ecs::resource::Resource;

/// 创造模式发放表：1 起始的稠密创造条目 id（与服务端 CreativeContent
/// 列表下发的 id 一致）到可发放物品堆模板（模板 count 恒为 1；实际发放
/// 数量由请求自身消耗 Output 的 Moves 在执行时决定）。
///
/// 创造载荷带 NBT 的条目只发放其基础物品：[`ItemStack`] 不携带 NBT，
/// 因此药水、附魔书等 NBT 附加信息暂不经创造发放往返。
///
/// Creative-mode grant table: dense 1-based creative entry ids (exactly the
/// ids sent in this server's CreativeContent listing) to grantable stack
/// templates (count is always 1; the granted amount comes from the
/// request's own Output-consuming Moves at execution).
///
/// Entries whose creative payload carries NBT grant their base item:
/// [`ItemStack`] carries no NBT, so NBT extras (potions, enchanted books)
/// do not round-trip through creative takes yet.
#[derive(Resource, Clone, Debug, Default)]
pub struct CreativeGrantTable {
    templates: Vec<ItemStack>,
}

impl CreativeGrantTable {
    /// 压入一个条目，返回其 1-based 创造 id。
    ///
    /// Pushes one entry, returning its 1-based creative id.
    pub fn push(&mut self, template: ItemStack) -> u32 {
        self.templates.push(template);
        self.templates.len() as u32
    }

    /// 若已列出，返回客户端发送的某个创造 id 的模板。
    ///
    /// Template for a creative id sent by the client, if listed.
    pub fn get(&self, creative_id: u32) -> Option<ItemStack> {
        creative_id
            .checked_sub(1)
            .and_then(|index| self.templates.get(index as usize))
            .cloned()
    }

    pub fn len(&self) -> usize {
        self.templates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }
}
