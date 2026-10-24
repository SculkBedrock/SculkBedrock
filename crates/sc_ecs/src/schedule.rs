//! `Schedule`：调度标签与调度表管理。
//!
//! 调度标签（`ScheduleLabel`）经驻留后作为键；`ScheduleManager` 按
//! First/PreUpdate/Update/PostUpdate/Last 主循环顺序与
//! PreStartup/Startup/PostStartup 启动顺序运行系统。
//!
//! `Schedule`: schedule labels and schedule-table management.
//!
//! Schedule labels (`ScheduleLabel`) are interned and used as keys;
//! `ScheduleManager` runs systems in the main-loop order
//! First/PreUpdate/Update/PostUpdate/Last and the startup order
//! PreStartup/Startup/PostStartup.

use crate::define_label;
use crate::intern::Interned;
use crate::system::{IntoSystem, System, SystemManager};
use ahash::{HashMap, HashMapExt};
pub use sc_ecs_macros::ScheduleLabel;

define_label!(ScheduleLabel, SCHEDULE_LABEL_INTERNER);

/// 主循环第一个调度标签。
///
/// The first main-loop schedule label.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct First;

/// 主循环更新前调度标签。
///
/// The pre-update main-loop schedule label.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct PreUpdate;

/// 固定步长主循环调度标签。
///
/// The fixed-timestep main-loop schedule label.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct RunFixedMainLoop;

/// 主循环更新调度标签。
///
/// The update main-loop schedule label.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Update;

/// 主循环更新后调度标签。
///
/// The post-update main-loop schedule label.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct PostUpdate;

/// 主循环最后一个调度标签。
///
/// The last main-loop schedule label.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Last;

/// 启动前调度标签。
///
/// The pre-startup schedule label.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct PreStartup;

/// 启动调度标签。
///
/// The startup schedule label.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Startup;

/// 启动后调度标签。
///
/// The post-startup schedule label.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct PostStartup;

/// 调度表：按标签持有系统集合与标签顺序。
///
/// Schedule table: holds system sets per label plus the label orderings.
pub struct ScheduleManager {
    /// 标签到系统集合的映射。
    ///
    /// Map from label to system set.
    schedules: HashMap<Interned<dyn ScheduleLabel>, SystemManager>,
    /// 主循环标签顺序。
    ///
    /// Main-loop label order.
    labels: Vec<Interned<dyn ScheduleLabel>>,
    /// 启动标签顺序。
    ///
    /// Startup label order.
    startup_labels: Vec<Interned<dyn ScheduleLabel>>,
    /// 是否已跑过首次（启动）调度。
    ///
    /// Whether the first (startup) run has happened.
    first_run: bool,
}

impl ScheduleManager {
    /// 创建调度表（含默认主循环/启动标签顺序）。
    ///
    /// Creates a schedule table with the default main-loop/startup
    /// label order.
    pub fn new() -> Self {
        Self {
            schedules: HashMap::new(),
            labels: vec![
                First.intern(),
                PreUpdate.intern(),
                RunFixedMainLoop.intern(),
                Update.intern(),
                PostUpdate.intern(),
                Last.intern(),
            ],
            startup_labels: vec![PreStartup.intern(), Startup.intern(), PostStartup.intern()],
            first_run: false,
        }
    }

    /// 在指定标签之前插入新标签；参照标签不存在时 panic。
    ///
    /// Inserts a new label before the given one; panics if the
    /// reference label does not exist.
    ///
    /// - `before`: 参照标签
    /// - `schedule`: 要插入的标签
    pub fn insert_before(&mut self, before: impl ScheduleLabel, schedule: impl ScheduleLabel) {
        let index = self
            .labels
            .iter()
            .position(|current| (**current).eq(&before))
            .unwrap_or_else(|| panic!("Expected {before:?} to exist"));
        self.labels.insert(index, schedule.intern());
    }

    /// 在指定标签之后插入新标签；参照标签不存在时 panic。
    ///
    /// Inserts a new label after the given one; panics if the reference
    /// label does not exist.
    ///
    /// - `after`: 参照标签
    /// - `schedule`: 要插入的标签
    pub fn insert_after(&mut self, after: impl ScheduleLabel, schedule: impl ScheduleLabel) {
        let index = self
            .labels
            .iter()
            .position(|current| (**current).eq(&after))
            .unwrap_or_else(|| panic!("Expected {after:?} to exist"));
        self.labels.insert(index + 1, schedule.intern());
    }

    /// 在指定启动标签之前插入新标签；参照标签不存在时 panic。
    ///
    /// Inserts a new startup label before the given one; panics if the
    /// reference label does not exist.
    ///
    /// - `before`: 参照启动标签
    /// - `schedule`: 要插入的标签
    pub fn insert_startup_before(
        &mut self,
        before: impl ScheduleLabel,
        schedule: impl ScheduleLabel,
    ) {
        let index = self
            .startup_labels
            .iter()
            .position(|current| (**current).eq(&before))
            .unwrap_or_else(|| panic!("Expected {before:?} to exist"));
        self.startup_labels.insert(index, schedule.intern());
    }

    /// 在指定启动标签之后插入新标签；参照标签不存在时 panic。
    ///
    /// Inserts a new startup label after the given one; panics if the
    /// reference label does not exist.
    ///
    /// - `after`: 参照启动标签
    /// - `schedule`: 要插入的标签
    pub fn insert_startup_after(
        &mut self,
        after: impl ScheduleLabel,
        schedule: impl ScheduleLabel,
    ) {
        let index = self
            .startup_labels
            .iter()
            .position(|current| (**current).eq(&after))
            .unwrap_or_else(|| panic!("Expected {after:?} to exist"));
        self.startup_labels.insert(index + 1, schedule.intern());
    }

    /// 向指定调度注册一个系统。
    ///
    /// Registers a system into the given schedule.
    ///
    /// - `schedule`: 目标调度标签
    /// - `system`: 要注册的系统源
    #[allow(dead_code)]
    pub(crate) fn add_systems<S, Marker>(&mut self, schedule: impl ScheduleLabel, system: S)
    where
        S: IntoSystem<S, Marker> + 'static,
        Marker: 'static,
    {
        self.schedules
            .entry(schedule.intern())
            .or_insert_with(SystemManager::new)
            .add_systems(system);
    }

    /// 注册一个已盒装的系统到指定标签。
    ///
    /// Registers an already-boxed system into the given label.
    ///
    /// - `label`: 目标调度标签
    /// - `system`: 要注册的系统
    pub(crate) fn add_boxed(
        &mut self,
        label: Interned<dyn ScheduleLabel>,
        system: Box<dyn System>,
    ) {
        self.schedules
            .entry(label)
            .or_insert_with(SystemManager::new)
            .add_boxed(system);
    }

    /// 本 tick 要运行的标签序列（首次运行为启动标签，其后为主循环标签）。
    ///
    /// Label sequence to run this tick (startup labels on the first run, main-loop labels after).
    pub(crate) fn plan(&mut self) -> Vec<Interned<dyn ScheduleLabel>> {
        if !self.first_run {
            self.first_run = true;
            self.startup_labels.clone()
        } else {
            self.labels.clone()
        }
    }

    /// 取出某个标签的系统集合（留下空占位）。
    ///
    /// Takes out one label's system set (leaving an empty placeholder).
    ///
    /// - `label`: 目标调度标签
    pub(crate) fn take_label(
        &mut self,
        label: &Interned<dyn ScheduleLabel>,
    ) -> Option<SystemManager> {
        self.schedules
            .get_mut(label)
            .map(|systems| std::mem::replace(systems, SystemManager::new()))
    }

    /// 归还取出的系统集合；运行期间注册到该标签的系统追加在末尾
    /// （下一个 tick 生效）。
    ///
    /// Returns the taken-out system set; systems registered to this label
    /// during the run are appended at the end (effective next tick).
    ///
    /// - `label`: 目标调度标签
    /// - `systems`: 要归还的系统集合
    pub(crate) fn restore_label(
        &mut self,
        label: Interned<dyn ScheduleLabel>,
        mut systems: SystemManager,
    ) {
        let entry = self
            .schedules
            .entry(label)
            .or_insert_with(SystemManager::new);
        let added_during_run = std::mem::replace(entry, SystemManager::new());
        systems.append(added_during_run);
        *entry = systems;
    }
}
