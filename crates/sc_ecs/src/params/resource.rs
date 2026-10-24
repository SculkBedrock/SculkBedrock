//! `Res`/`ResMut` 参数：只读与独占可写的资源守卫。
//!
//! `Res<T>` 是持有式读守卫，`ResMut<T>` 是持有式写守卫；两者都可跨 await
//! 持有。资源缺失时参数提取返回 `None`，本次系统执行被跳过。
//!
//! The `Res`/`ResMut` parameters: read-only and exclusive writable resource
//! guards.
//!
//! `Res<T>` is an owned read guard and `ResMut<T>` an owned write guard; both
//! may be held across await. When the resource is absent the parameter
//! extraction returns `None` and the system run is skipped.
use crate::resource::Resource;
use crate::system::{SystemParam, SystemParamState};
use crate::world::World;
use parking_lot::{ArcRwLockReadGuard, ArcRwLockWriteGuard, RawRwLock};
use sc_error::{ScError, ScSite, capture_site};
use std::fmt::{Debug, Formatter};
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use sc_log::t_log;

/// 资源访问相关错误。
///
/// Errors raised while accessing resources.
#[derive(ScError)]
pub enum ResourceError {
    /// 请求的资源在世界中不存在。
    ///
    /// Requested resource is absent from the world.
    #[sc_error(code = "ecs.resource.not_found")]
    NotFound {
        /// 缺失的资源名称。
        ///
        /// Missing resource name.
        name: String,
        /// 说明成因的详细描述。
        ///
        /// Detail explaining the cause.
        detail: String,
        /// 错误构造位置。
        ///
        /// Construction site.
        site: ScSite,
    },
}

impl ResourceError {
    /// 构造一个资源缺失错误，并记录调用位置。
    ///
    /// Builds a missing-resource error, capturing the call site.
    ///
    /// - `name`: 缺失的资源名称
    #[track_caller]
    pub fn not_found(name: String) -> Self {
        let detail = format!("Resource({name}) not found");
        Self::NotFound {
            name,
            detail,
            site: capture_site(),
        }
    }
}

impl std::fmt::Display for ResourceError {
    /// 以错误码 + 详情 + 位置的形式输出错误。
    ///
    /// Formats the error as code + detail + site.
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { detail, site, .. } => {
                write!(f, "[ecs.resource.not_found] {detail} (at {site})")
            }
        }
    }
}

impl Debug for ResourceError {
    /// 以简洁形式输出错误（仅资源名）。
    ///
    /// Formats the error concisely (resource name only).
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { name, .. } => write!(f, "Resource({name}) not found"),
        }
    }
}

impl std::error::Error for ResourceError {}

/// 共享只读句柄：持有式读守卫；同一线程上递归重读同一资源不会与排队的
/// 写者死锁。可跨 await 持有；持有期间写者会等待，不宜长时间持有热点
/// 资源的 `Res`。
///
/// Shared read handle: an owned read guard; re-reading the same resource
/// recursively on one thread never deadlocks against a queued writer. May be
/// held across await; writers wait while it is held, so avoid holding `Res`
/// of hot resources for long.
pub struct Res<T: 'static + Resource + Send + Sync> {
    /// 底层读守卫。
    ///
    /// The underlying read guard.
    pub(crate) guard: ArcRwLockReadGuard<RawRwLock, T>,
}

impl<T: 'static + Resource + Send + Sync> Res<T> {
    /// 由读守卫构造 `Res`。
    ///
    /// Constructs a `Res` from a read guard.
    ///
    /// - `guard`: 底层读守卫
    pub(crate) fn new(guard: ArcRwLockReadGuard<RawRwLock, T>) -> Self {
        Self { guard }
    }
}

impl<T: 'static + Resource + Send + Sync> Deref for Res<T> {
    type Target = T;

    /// 解引用到被保护的资源。
    ///
    /// Dereferences to the guarded resource.
    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

/// `Res<T>` 的参数状态（无状态）。
///
/// The parameter state of `Res<T>` (stateless).
pub struct ParamResState<T> {
    /// 资源类型占位。
    ///
    /// Phantom data for the resource type.
    _phantom: PhantomData<T>,
}

impl<T: 'static + Resource + Send + Sync> SystemParamState for ParamResState<T> {
    type Item = Res<T>;

    /// 初始化参数状态。
    ///
    /// Initializes the parameter state.
    fn init() -> Self {
        Self {
            _phantom: Default::default(),
        }
    }

    /// 提取本次执行的资源只读守卫；资源缺失时记录错误并返回 `None`。
    ///
    /// Extracts the read guard for this run; logs an error and returns `None`
    /// when the resource is absent.
    fn get_param<'a>(
        _state: &'a mut Self,
        world: &'a World,
    ) -> Option<<<Self as SystemParamState>::Item as SystemParam>::This<'a>> {
        let resource = world.get_resource();
        if resource.is_none() {
            log::error!(
                "{}",
                t_log!("console.ecs.skipped_resource", name = T::name())
            );
        }
        resource
    }
}

/// `Res<T>` 可作为系统参数注入。
///
/// `Res<T>` can be injected as a system parameter.
unsafe impl<T: 'static + Resource + Send + Sync> SystemParam for Res<T> {
    type This<'a> = Res<T>;
    type State = ParamResState<T>;
}

/// 独占写句柄：持有式写守卫；持有期间该资源的所有读写都会等待。
/// 在持有 `ResMut` 期间再获取同一资源的 `Res`/`ResMut`（包括通过 `World`
/// 重新获取）会死锁。
///
/// Exclusive write handle: an owned write guard. All reads and writes of this
/// resource wait while it is held. Acquiring the same resource again (through
/// `World` or as `Res`/`ResMut` in one system) while holding `ResMut`
/// deadlocks.
pub struct ResMut<T: 'static + Resource + Send + Sync> {
    /// 底层写守卫。
    ///
    /// The underlying write guard.
    pub(crate) guard: ArcRwLockWriteGuard<RawRwLock, T>,
}

impl<T: 'static + Resource + Send + Sync> ResMut<T> {
    /// 由写守卫构造 `ResMut`。
    ///
    /// Constructs a `ResMut` from a write guard.
    ///
    /// - `guard`: 底层写守卫
    pub(crate) fn new(guard: ArcRwLockWriteGuard<RawRwLock, T>) -> Self {
        Self { guard }
    }
}

impl<T: 'static + Resource + Send + Sync> Deref for ResMut<T> {
    type Target = T;

    /// 解引用到被保护的资源。
    ///
    /// Dereferences to the guarded resource.
    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl<T: 'static + Resource + Send + Sync> DerefMut for ResMut<T> {
    /// 可变解引用到被保护的资源。
    ///
    /// Mutably dereferences to the guarded resource.
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

/// `ResMut<T>` 的参数状态（无状态）。
///
/// The parameter state of `ResMut<T>` (stateless).
pub struct ParamResMutState<T> {
    /// 资源类型占位。
    ///
    /// Phantom data for the resource type.
    _phantom: PhantomData<T>,
}

impl<T: 'static + Resource + Send + Sync> SystemParamState for ParamResMutState<T> {
    type Item = ResMut<T>;

    /// 初始化参数状态。
    ///
    /// Initializes the parameter state.
    fn init() -> Self {
        Self {
            _phantom: Default::default(),
        }
    }

    /// 提取本次执行的资源写守卫；资源缺失时记录错误并返回 `None`。
    ///
    /// Extracts the write guard for this run; logs an error and returns `None`
    /// when the resource is absent.
    fn get_param<'a>(
        _state: &'a mut Self,
        world: &'a World,
    ) -> Option<<<Self as SystemParamState>::Item as SystemParam>::This<'a>> {
        let resource = world.get_resource_mut();
        if resource.is_none() {
            log::error!(
                "{}",
                t_log!("console.ecs.skipped_mut_resource", name = T::name())
            );
        }
        resource
    }
}

/// `ResMut<T>` 可作为系统参数注入。
///
/// `ResMut<T>` can be injected as a system parameter.
unsafe impl<T: 'static + Resource + Send + Sync> SystemParam for ResMut<T> {
    type This<'a> = ResMut<T>;
    type State = ParamResMutState<T>;
}
