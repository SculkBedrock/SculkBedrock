//! Material filler rule and multi-material combinator.
//!
//! Block states are runtime IDs; nullable states are Options.
//! Block states are runtime IDs; nullable states are Options. //

use crate::worldgen::densityfunction::function::FunctionContext;
use std::sync::Arc;
use sc_world::chunk::BlockRuntimeId;

/// 根据采样上下文计算方块材料的接口。
///
/// Interface computing a block material from a sampling context.
pub trait MaterialFiller {
    /// Java: `@Nullable BlockState calculate(FunctionContext)`(L11).
    fn calculate(&self, context: &dyn FunctionContext) -> Option<BlockRuntimeId>;
}

/// 按顺序尝试多个填充器并返回第一个非空结果的填充器。
///
/// Filler trying multiple fillers in order and returning the first non-null result.
pub struct MultiMaterial {
    pub materials: Vec<Arc<dyn MaterialFiller>>,
}

impl MultiMaterial {
    /// Builds a multi-material from a rule list.
    pub fn new(materials: Vec<Arc<dyn MaterialFiller>>) -> Self {
        Self { materials }
    }
}

impl MaterialFiller for MultiMaterial {
    /// Returns the first non-empty result in order.
    fn calculate(&self, context: &dyn FunctionContext) -> Option<BlockRuntimeId> {
        for rule in &self.materials {
            if let Some(state) = rule.calculate(context) {
                return Some(state);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::densityfunction::function::SinglePointContext;

    /// 测试用的固定输出填充器。
    ///
    /// Test filler returning a fixed output.
    struct Fixed(Option<BlockRuntimeId>);

    impl MaterialFiller for Fixed {
        fn calculate(&self, _context: &dyn FunctionContext) -> Option<BlockRuntimeId> {
            self.0
        }
    }

    fn ctx() -> SinglePointContext {
        SinglePointContext {
            block_x: 0,
            block_y: 0,
            block_z: 0,
        }
    }

    #[test]
    fn multi_material_returns_first_non_none() {
        let multi = MultiMaterial::new(vec![
            Arc::new(Fixed(None)),
            Arc::new(Fixed(Some(BlockRuntimeId(7)))),
            Arc::new(Fixed(Some(BlockRuntimeId(9)))),
        ]);
        assert_eq!(multi.calculate(&ctx()), Some(BlockRuntimeId(7)));
    }

    #[test]
    fn multi_material_all_none_returns_none() {
        let all_none = MultiMaterial::new(vec![Arc::new(Fixed(None)), Arc::new(Fixed(None))]);
        assert_eq!(all_none.calculate(&ctx()), None);
    }

    #[test]
    fn multi_material_empty_returns_none() {
        let empty = MultiMaterial::new(vec![]);
        assert_eq!(empty.calculate(&ctx()), None);
    }
}
