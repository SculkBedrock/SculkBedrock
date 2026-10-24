//! `System`：系统 trait 与 `SystemParam` 注入。
//!
//! 系统是 ECS 的执行单元：`run(&mut self, world)` 从 World 消费
//! 组件/资源/事件。`SystemParam` 支持 Res/ResMut/World/Components/Query/
//! EventReader/Local 参数；`async_system` 宏将系统放到共享 tokio 运行时上执行。
//!
//! `System`: the system trait plus `SystemParam` injection.
//!
//! A system is the ECS execution unit: `run(&mut self, world)` consumes
//! components/resources/events from the World. `SystemParam` supports
//! Res/ResMut/World/Components/Query/EventReader/Local parameters; the
//! `async_system` macro runs a system on the shared tokio runtime.

use crate::world::World;
use crate::{
    impl_system_function, impl_system_param_state_tuple, impl_system_param_tuple,
    impl_tuple_system_function,
};
use std::marker::PhantomData;
use sc_ecs_macros::all_tuples;

/// 系统 trait：ECS 的执行单元。
///
/// System trait: the execution unit of the ECS.
pub trait System: Send + Sync {
    /// 执行一次系统逻辑。
    ///
    /// Runs the system logic once.
    ///
    /// - `world`: 系统读取状态的 World
    fn run(&mut self, world: &World);
}

impl System for () {
    /// 空实现：不执行任何逻辑。
    ///
    /// A no-op implementation that performs no logic.
    fn run(&mut self, _world: &World) {}
}

/// 系统参数 trait：可从 World 提取的注入参数。
///
/// SystemParam trait: an injected parameter that can be extracted from
/// the World.
pub unsafe trait SystemParam: Send + Sync {
    /// 参数的存活类型（通常是与 World 借用绑定的引用）。
    ///
    /// The live type of the parameter (usually a reference tied to the
    /// World borrow).
    type This<'a>;
    /// 参数的持久化状态。
    ///
    /// The persistent state of the parameter.
    type State: SystemParamState<Item = Self>;
}

all_tuples!(impl_system_param_tuple, 0, 20, P, p);

/// 系统参数状态 trait：管理 `SystemParam` 的初始化与提取。
///
/// SystemParamState trait: manages initialization and extraction of a
/// `SystemParam`.
pub trait SystemParamState: Send + Sync {
    /// 本状态对应的参数类型。
    ///
    /// The parameter type corresponding to this state.
    type Item: SystemParam<State = Self>;

    /// 初始化参数状态。
    ///
    /// Initializes the parameter state.
    fn init() -> Self;

    /// 从状态与 World 提取本次执行的参数；资源缺失等情况下返回 `None`
    /// （系统被跳过）。
    ///
    /// Extracts the parameter for this run from the state and World;
    /// returns `None` (skipping the system) when e.g. a resource is
    /// missing.
    ///
    /// - `state`: 参数状态的可变引用
    /// - `world`: 提取来源的 World
    fn get_param<'a>(
        state: &'a mut Self,
        world: &'a World,
    ) -> Option<<<Self as SystemParamState>::Item as SystemParam>::This<'a>>;
}

all_tuples!(impl_system_param_state_tuple, 0, 20, P, p);

/// 系统转换 trait：把函数/闭包等转换为具体系统类型。
///
/// IntoSystem trait: converts functions/closures etc. into a concrete
/// system type.
pub trait IntoSystem<S, Marker> {
    /// 转换得到的系统类型。
    ///
    /// The resulting system type.
    type System: System;

    /// 执行转换。
    ///
    /// Performs the conversion.
    fn into_system(self) -> Self::System;
}

impl<S, Marker> IntoSystem<S, Marker> for S
where
    Marker: 'static + Send + Sync,
    S: SystemFunction<Marker>,
{
    type System = FunctionSystem<Marker, S>;

    /// 将函数/闭包包装为 `FunctionSystem`。
    ///
    /// Wraps the function/closure into a `FunctionSystem`.
    fn into_system(self) -> Self::System {
        FunctionSystem::new(self)
    }
}

/// 系统函数 trait：可被 `FunctionSystem` 驱动的函数/闭包。
///
/// SystemFunction trait: a function/closure that can be driven by a
/// `FunctionSystem`.
pub trait SystemFunction<Marker>: 'static + Send + Sync {
    /// 函数提取的参数类型。
    ///
    /// The parameter type extracted by the function.
    type Param: SystemParam;

    /// 以提取出的参数执行一次函数。
    ///
    /// Runs the function once with the extracted parameters.
    ///
    /// - `param`: 本次执行的参数
    fn run(&mut self, param: <Self::Param as SystemParam>::This<'_>);
}

all_tuples!(impl_system_function, 0, 20, P, p);

/// 函数系统：把一个 `SystemFunction` 连同其参数状态包装为 `System`。
///
/// FunctionSystem: wraps a `SystemFunction` together with its parameter
/// state into a `System`.
pub struct FunctionSystem<Marker, F: SystemFunction<Marker>> {
    /// 被驱动的函数。
    ///
    /// The wrapped function.
    func: F,
    /// 参数的持久化状态。
    ///
    /// The persistent parameter state.
    state: <F::Param as SystemParam>::State,
    /// Marker 类型占位。
    ///
    /// Phantom data for the marker type.
    _phantom_data: PhantomData<Marker>,
}

impl<Marker, F> FunctionSystem<Marker, F>
where
    F: SystemFunction<Marker>,
{
    /// 由函数创建一个函数系统（参数状态初始化为默认值）。
    ///
    /// Creates a function system from a function (parameter state is
    /// initialized to its default).
    ///
    /// - `func`: 要驱动的函数
    pub fn new(func: F) -> Self {
        Self {
            func,
            state: <F::Param as SystemParam>::State::init(),
            _phantom_data: Default::default(),
        }
    }
}

impl<Marker: Send + Sync, F: SystemFunction<Marker>> System for FunctionSystem<Marker, F> {
    /// 提取本次执行的参数并驱动被包装的函数；参数不可用时跳过本次执行。
    ///
    /// Extracts the parameters for this run and drives the wrapped function;
    /// skips the run when the parameters are unavailable.
    ///
    /// - `world`: 系统读取状态的 World
    fn run(&mut self, world: &World) {
        if let Some(param) = <F::Param as SystemParam>::State::get_param(&mut self.state, world) {
            self.func.run(param);
        }
    }
}

/// 元组系统：按顺序依次运行多个系统。
///
/// TupleFunctionSystem: runs several systems in order.
pub struct TupleFunctionSystem {
    /// 被依次运行的系统列表。
    ///
    /// The list of systems to run in order.
    systems: Vec<Box<dyn System>>,
}

impl TupleFunctionSystem {
    /// 由系统列表创建元组系统。
    ///
    /// Creates a tuple system from a list of systems.
    ///
    /// - `systems`: 要依次运行的系统
    pub fn new(systems: Vec<Box<dyn System>>) -> Self {
        Self { systems }
    }
}

impl System for TupleFunctionSystem {
    /// 按列表顺序依次运行每个子系统。
    ///
    /// Runs each sub-system in list order.
    ///
    /// - `world`: 系统读取状态的 World
    fn run(&mut self, world: &World) {
        for system in self.systems.iter_mut() {
            system.run(world);
        }
    }
}

all_tuples!(impl_tuple_system_function, 2, 20, P, p, M);

/// 系统管理器：按注册顺序持有并运行系统。
///
/// SystemManager: holds and runs systems in registration order.
pub struct SystemManager {
    /// 系统列表。
    ///
    /// The list of systems.
    systems: Vec<Box<dyn System>>,
}

impl SystemManager {
    /// 创建一个空的系统管理器。
    ///
    /// Creates an empty system manager.
    pub fn new() -> Self {
        Self { systems: vec![] }
    }

    /// 注册一个可转换为系统的函数/闭包。
    ///
    /// Registers a function/closure that converts into a system.
    ///
    /// - `system`: 要注册的系统源
    pub fn add_systems<S, Marker>(&mut self, system: S)
    where
        S: IntoSystem<S, Marker> + 'static,
        Marker: 'static,
    {
        self.systems.push(Box::new(system.into_system()));
    }

    /// 注册一个已盒装的系统。
    ///
    /// Registers an already-boxed system.
    ///
    /// - `system`: 要注册的系统
    pub fn add_boxed(&mut self, system: Box<dyn System>) {
        self.systems.push(system);
    }

    /// 追加另一个系统集合中的全部系统（保持顺序）。
    ///
    /// Appends every system from another set (order preserved).
    ///
    /// - `other`: 来源系统集合（被移动）
    pub fn append(&mut self, other: SystemManager) {
        self.systems.extend(other.systems);
    }

    /// 按注册顺序运行全部系统。
    ///
    /// Runs all systems in registration order.
    ///
    /// - `world`: 系统读取状态的 World
    pub fn run(&mut self, world: &World) {
        for system in self.systems.iter_mut() {
            system.run(world);
        }
    }
}
