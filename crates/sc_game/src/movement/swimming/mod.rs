//! 玩家游泳子系统。
//!
//! Player swimming subsystem.
//!
//! 负责流体浸润探测、游泳状态机流转、出水即刻撤销（Zero Water Revocation）
//! 以及 1 格高空间爬行兼容。

pub mod immersion;

pub use immersion::{
    detect_immersion, has_headroom, is_water_block_at, is_water_fluid_name, ImmersionDepth,
};

use sc_ecs::component::Component;
use sc_ecs::entity::EntityId;
use sc_ecs::world::World;

/// 实体游泳状态组件（纯数据 ECS 组件）。
///
/// Player swimming state component.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Swimming {
    /// 当前是否处于游泳姿态。
    pub is_swimming: bool,
    /// 连续脱离水面的 tick 数。
    pub air_ticks: u32,
    /// 是否受限于上方 1 格方块被迫处于爬行/趴下状态。
    pub is_crawling: bool,
}

/// 兼容性类型别名：Swimming 组件内部状态。
pub type SwimmingState = Swimming;

impl Swimming {
    /// 构造新的 Swimming 组件。
    pub fn new(is_swimming: bool) -> Self {
        Self {
            is_swimming,
            air_ticks: 0,
            is_crawling: false,
        }
    }

    /// 当前是否处于游泳状态。
    #[inline]
    pub fn is_swimming(&self) -> bool {
        self.is_swimming
    }

    /// 设置游泳状态。
    #[inline]
    pub fn set_swimming(&mut self, is_swimming: bool) {
        self.is_swimming = is_swimming;
        if !is_swimming {
            self.is_crawling = false;
        }
    }
}

/// 确保实体拥有 `Swimming` 组件。不存在时创建，存在时返回其副本。
pub fn get_or_create_swimming(world: &World, entity: &EntityId) -> Option<Swimming> {
    match world.get_component::<Swimming>(entity) {
        Some(swimming) => Some(*swimming.as_ref()),
        None => {
            world.add_component(entity, Swimming::default());
            world.get_component::<Swimming>(entity).map(|s| *s.as_ref())
        }
    }
}

/// 游泳状态机决策函数（纯函数）。
///
/// 根据当前游泳状态、水体浸润深度、客户端显式输入与站立空间，
/// 决定下一时刻的 `Swimming` 状态。
pub fn evaluate_swimming_transition(
    current: Swimming,
    immersion: ImmersionDepth,
    req_start: bool,
    req_stop: bool,
    can_stand: bool,
) -> Swimming {
    if !current.is_swimming {
        // 未在游泳：只有处于深水中（Torso 或 Submerged）且收到显式 start_swimming 请求才进入
        if req_start && immersion.can_swim() {
            Swimming {
                is_swimming: true,
                air_ticks: 0,
                is_crawling: false,
            }
        } else {
            Swimming {
                is_swimming: false,
                air_ticks: 0,
                is_crawling: false,
            }
        }
    } else {
        // 正在游泳中：
        // 1. 若实体已经脱离可游泳水体（无水或仅脚底浅水）
        if immersion.is_out_of_water() {
            if can_stand {
                // 上方有空间完全站立：无条件立刻撤销游泳状态！
                Swimming {
                    is_swimming: false,
                    air_ticks: current.air_ticks.saturating_add(1),
                    is_crawling: false,
                }
            } else {
                // 上方受阻（如 1 格高的隧道/方块狭缝）：保留 0.6 趴伏姿态转换为爬行
                Swimming {
                    is_swimming: true,
                    air_ticks: current.air_ticks.saturating_add(1),
                    is_crawling: true,
                }
            }
        } else if req_stop {
            // 2. 玩家在水中主动请求停止游泳
            if can_stand {
                Swimming {
                    is_swimming: false,
                    air_ticks: 0,
                    is_crawling: false,
                }
            } else {
                // 水中狭缝同样保持趴伏
                Swimming {
                    is_swimming: true,
                    air_ticks: 0,
                    is_crawling: true,
                }
            }
        } else {
            // 3. 正常水中游动
            Swimming {
                is_swimming: true,
                air_ticks: 0,
                is_crawling: false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_swimming_requires_deep_water_and_explicit_request() {
        let current = Swimming::default();

        // 1. 无水 + req_start -> 无法游泳
        let next = evaluate_swimming_transition(current, ImmersionDepth::None, true, false, true);
        assert!(!next.is_swimming);

        // 2. 浅水（FeetOnly）+ req_start -> 无法游泳
        let next =
            evaluate_swimming_transition(current, ImmersionDepth::FeetOnly, true, false, true);
        assert!(!next.is_swimming);

        // 3. 躯干浸水（Torso）+ 无 req_start -> 保持不游
        let next = evaluate_swimming_transition(current, ImmersionDepth::Torso, false, false, true);
        assert!(!next.is_swimming);

        // 4. 躯干浸水（Torso）+ req_start -> 成功开始游泳
        let next = evaluate_swimming_transition(current, ImmersionDepth::Torso, true, false, true);
        assert!(next.is_swimming);
        assert!(!next.is_crawling);

        // 5. 完全沉浸（Submerged）+ req_start -> 成功开始游泳
        let next =
            evaluate_swimming_transition(current, ImmersionDepth::Submerged, true, false, true);
        assert!(next.is_swimming);
    }

    #[test]
    fn zero_water_revokes_swimming_immediately_when_can_stand() {
        let current = Swimming::new(true);

        // 离开水面，进入空气 (ImmersionDepth::None)，且上方有空间站立 (can_stand=true)
        let next = evaluate_swimming_transition(current, ImmersionDepth::None, false, false, true);
        assert!(
            !next.is_swimming,
            "swimming must be immediately revoked when out of water"
        );
        assert_eq!(next.air_ticks, 1);
        assert!(!next.is_crawling);

        // 仅脚底沾水 (FeetOnly) 冲出水面跃向空气
        let next_feet =
            evaluate_swimming_transition(current, ImmersionDepth::FeetOnly, false, false, true);
        assert!(
            !next_feet.is_swimming,
            "feet-only water must revoke swimming when jumping into air"
        );
    }

    #[test]
    fn zero_water_enters_crawling_when_cannot_stand() {
        let current = Swimming::new(true);

        // 游入 1 格高陆地隧道（无水，但 can_stand=false）
        let next = evaluate_swimming_transition(current, ImmersionDepth::None, false, false, false);
        assert!(
            next.is_swimming,
            "player must remain in 0.6m prone pose when under a solid ceiling"
        );
        assert!(next.is_crawling, "must be marked as crawling");
    }

    #[test]
    fn stop_swimming_request_in_water_exits_when_can_stand() {
        let current = Swimming::new(true);

        // 水中收到主动 stop 请求
        let next = evaluate_swimming_transition(current, ImmersionDepth::Torso, false, true, true);
        assert!(!next.is_swimming);

        // 若水中上方是 1 格低顶，收到 stop 依然保持趴伏
        let next_under_slab =
            evaluate_swimming_transition(current, ImmersionDepth::Torso, false, true, false);
        assert!(next_under_slab.is_swimming);
        assert!(next_under_slab.is_crawling);
    }

    #[test]
    fn get_or_create_swimming_on_ecs_world() {
        let world = World::new();
        let entity = world.spawn(sc_entity::MinecraftEntityId(1));

        assert!(world.get_component::<Swimming>(&entity).is_none());
        let s = get_or_create_swimming(&world, &entity).expect("should create component");
        assert!(!s.is_swimming);

        // Add swimming=true
        world.add_component(&entity, Swimming::new(true));
        let s2 = get_or_create_swimming(&world, &entity).expect("should reuse component");
        assert!(s2.is_swimming);
    }
}
