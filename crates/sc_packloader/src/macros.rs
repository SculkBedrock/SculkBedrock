/// 注册组件键：由 [`components_export!`] 为列表中的每个组件自动实现，
/// 使该列表保持唯一事实来源（无需每类型 derive 或手写 `impl`）。
///
/// Registered component key: implemented automatically by
/// [`components_export!`] for every listed component, so the list stays the
/// single source of truth (no per-type derive or hand-written `impl`).
pub trait ComponentKey {
    /// 注册的组件键（如 `"minecraft:food"`）。
    ///
    /// Registered component key (e.g. `"minecraft:food"`).
    const NAME: &'static str;
}

/// 组件导出宏：定义组件容器枚举，并为每个组件注册键（`ComponentKey`）、
/// Debug/查询/插入等实现。
///
/// Component export macro: defines the component container enum and
/// registers keys (`ComponentKey`), Debug/query/insert impls for each
/// component.
///
/// - `$struct_name`: 生成的容器枚举名
/// - `$name = $alias`: 组件类型名 = 注册键（如 `Food = "minecraft:food"`）
#[macro_export]
macro_rules! components_export {
    ($struct_name:ident, $($name:ident = $alias:expr,)*) => {
        // Defines the primary struct.
        #[allow(non_snake_case)]
        #[allow(unreachable_patterns)]
        #[allow(unused_variables)]
        /// 组件容器：保存解析出的组件键值对。
        ///
        /// Component container: holds the parsed component key/value pairs.
        #[derive(Deserialize, Clone)]
        #[serde(untagged)]
        pub enum $struct_name {
            /// 组件键到组件值的映射。
            ///
            /// A map from component key to component value.
            Map(Map<String, Value>)
        }
        $(
            impl $crate::macros::ComponentKey for $name {
                const NAME: &'static str = $alias;
            }
        )*
        impl std::fmt::Debug for $struct_name {
            /// 仅打印已注册组件，跳过未知键。
            ///
            /// Prints only registered components, skipping unknown keys.
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let mut f = f.debug_struct(stringify!($struct_name));
                match self {
                    $struct_name::Map(map) => {
                        for (key, value) in map {
                            match key.as_str() {
                                $(
                                    $alias => {
                                        if let Ok(component) = serde_json::from_value::<$name>(value.clone()) {
                                            f.field(stringify!($name), &component);
                                        }
                                    },
                                )*
                                _ => {}
                            }
                        }
                    },
                }
                f.finish()
            }
        }
        impl $struct_name {
            /// 全部已注册组件的键（别名），按声明顺序。
            ///
            /// Component key (alias) of every registered component, in declaration order.
            pub const NAMES: &'static [&'static str] = &[$($alias),*];

            /// 该键是否为已注册组件（供编译路由使用）。
            ///
            /// Whether the key is a registered component (used by compilation routing).
            pub fn is_known(key: &str) -> bool {
                Self::NAMES.contains(&key)
            }

            /// 用单个组件构造容器；别名未知时返回 `None`。
            ///
            /// Builds a container from a single component; `None` when the
            /// alias is unknown.
            pub fn new_single<T: serde::Serialize + sc_ecs::component::Component>(component: T) -> Option<Self> {
                let mut map = Map::new();
                let alias = Self::get_alias(&T::name())?;
                map.insert(alias, serde_json::to_value(component).ok()?);
                return Some(Self::Map(map));
            }

            /// 将容器内全部已注册组件挂载到指定实体；未知键或解析失败仅告警。
            ///
            /// Attaches every registered component in the container to the
            /// entity; unknown keys or parse failures only warn.
            pub fn insert(self, world: &sc_ecs::world::World, entity: &sc_ecs::entity::EntityId) {
                match self {
                    $struct_name::Map(map) => {
                        for (key, value) in map {
                            match key.as_str() {
                                $(
                                    $alias => {
                                        match serde_json::from_value::<$name>(value) {
                                            Ok(component) => {
                                                world.add_component(entity, component);
                                            }
                                            Err(error) => {
                                                log::warn!(
                                                    "{}",
                                                    sc_log::t_log!(
                                                        "console.pack.component_fail",
                                                        target = stringify!($struct_name),
                                                        alias = $alias,
                                                        error = error
                                                    )
                                                );
                                            }
                                        }
                                    },
                                )*
                                _ => {}
                            }
                        }
                    },
                }
            }


            /// 按 Rust 类型名查询注册键；未注册返回 `None`。
            ///
            /// Looks up the registered key by Rust type name; `None` when
            /// unregistered.
            pub fn get_alias(struct_name: &String) -> Option<String> {
                $(
                if struct_name == stringify!($name) {
                    return Some($alias.to_string());
                }
                )*
                None
            }

            /// 按键取出组件副本；未注册或解析失败返回 `None`。
            ///
            /// Takes a copy of the component by key; `None` when unregistered
            /// or unparseable.
            pub fn get<T: sc_ecs::component::Component>(&self) -> Option<T>
            where
                T: serde::de::DeserializeOwned,
            {
                let temp = Self::get_alias(&T::name())?;
                match self {
                    $struct_name::Map(map) => {
                        for (key, value) in map {
                            if key == &temp {
                                return serde_json::from_value::<T>(value.clone()).ok();
                            }
                        }
                        return None;
                    }
                }
            }

            /// 按键写入组件（覆盖同键旧值）；别名未知或序列化失败返回 `None`。
            ///
            /// Writes a component by key (overwriting the same key); `None`
            /// when the alias is unknown or serialization fails.
            pub fn push<T: serde::Serialize + sc_ecs::component::Component>(&mut self, component: T) -> Option<()> {
                match self {
                    $struct_name::Map(map) => {
                        let alias = Self::get_alias(&T::name())?;
                        map.insert(alias, serde_json::to_value(component).ok()?);
                    }
                }
                return Some(())
            }
        }
    };
}

/// 标识符枚举导出宏：生成“字符串变体”（可选加结构化变体）的标识符枚举
/// 及其 `Default`。
///
/// Identifier enum export macro: generates an ident enum with a string
/// variant (plus an optional structured variant) and its `Default`.
///
/// - `$enum_name`: 生成的枚举名
/// - `$variant`: 字符串变体名
/// - `default = $default`: `Default` 使用的回退名称
#[macro_export]
macro_rules! define_ident {
    // Single string-variant ident (item, entity): default is the empty string.
    ($enum_name:ident, $variant:ident) => {
        /// 标识符枚举：仅含字符串变体。
        ///
        /// Identifier enum: string variant only.
        #[derive(::serde::Deserialize, Debug, Clone)]
        #[serde(untagged)]
        pub enum $enum_name {
            /// 标识符的字符串形式。
            ///
            /// The identifier in string form.
            $variant(String),
        }
        impl Default for $enum_name {
            /// 默认标识符：空字符串。
            ///
            /// Default identifier: the empty string.
            fn default() -> Self {
                Self::$variant(String::new())
            }
        }
    };
    // String-variant plus structured-variant ident (block):
    // `default = "..."` is the fallback name used by `Default`.
    ($enum_name:ident, $variant:ident, $structured_variant:ident($structured_ty:ty), default = $default:expr) => {
        /// 标识符枚举：含字符串变体与结构化变体。
        ///
        /// Identifier enum: a string variant plus a structured variant.
        #[derive(::serde::Deserialize, Debug, Clone)]
        #[serde(untagged)]
        pub enum $enum_name {
            /// 标识符的字符串形式。
            ///
            /// The identifier in string form.
            $variant(String),
            /// 标识符的结构化形式。
            ///
            /// The identifier in structured form.
            $structured_variant($structured_ty),
        }
        impl Default for $enum_name {
            /// 默认标识符：指定的回退名称。
            ///
            /// Default identifier: the configured fallback name.
            fn default() -> Self {
                Self::$variant($default.to_string())
            }
        }
    };
}

/// 顶层 JSON 类型导出宏：生成“某个 spawner 的 Option 字段”容器结构体、
/// 分发枚举与 `get` 分发方法。
///
/// Top-level JSON type export macro: generates the container struct with an
/// Option field per spawner, the dispatch enum, and the `get` dispatcher.
///
/// - `$struct_name`: 生成的容器结构体名
/// - `$enum_name`: 生成的分发枚举名
/// - `$name = $alias`: 字段名 = JSON 键（如 `MinecraftEntitySpawner = "minecraft:entity"`）
#[macro_export]
macro_rules! types_export {
    ($struct_name:ident, $enum_name:ident, $($name:ident = $alias:expr,)*) => {
        // Main struct definition.
        /// spawner 分发枚举：标识实际存在的 spawner 类型。
        ///
        /// Spawner dispatch enum: identifies which spawner type is present.
        pub enum $enum_name {
            /// 未找到任何 spawner。
            ///
            /// No spawner was found.
            Empty,
            $(
                /// 具体的 spawner 类型。
                ///
                /// The concrete spawner type.
                $name($name),
            )*
        }
        /// spawner 容器：每个 spawner 一个可选字段。
        ///
        /// Spawner container: one optional field per spawner.
        #[allow(non_snake_case)]
        #[derive(Deserialize, Debug)]
        pub struct $struct_name {
            /// 资源格式版本号。
            ///
            /// Resource format version.
            pub format_version: String,
            $(
                #[serde(alias = $alias)]
                /// 对应 spawner 的数据。
                ///
                /// The data of the corresponding spawner.
                pub $name: Option<$name>,
            )*
        }
        impl $struct_name {
            /// 取出首个存在的 spawner（回填 format_version）；都没有则为 `Empty`。
            ///
            /// Takes the first present spawner (backfilling format_version);
            /// `Empty` when none is present.
            pub fn get(self) -> $enum_name {
                $(
                    if let Some(mut component) = self.$name {
                        component.format_version = self.format_version;
                        return $enum_name::$name(component);
                    }
                )*
                return $enum_name::Empty;
            }
        }
    };
}

/// 生物群系组件 NBT 导出宏：为组件容器生成 `to_nbt`（含 Climate 特例处理）。
///
/// Biome component NBT export macro: generates `to_nbt` for the component
/// container (including the `Climate` special case).
///
/// - `$struct_name`: 组件容器枚举名
/// - `$name`: 参与导出的组件类型名
#[macro_export]
macro_rules! biome_nbt_export {
    ($struct_name:ident, $($name:ident),*) => {
        impl $struct_name {
            /// 将各已注册组件序列化为复合 NBT；别名缺失或组件无 NBT 时报错。
            ///
            /// Serializes each registered component into a compound NBT;
            /// errors on a missing alias or a component without NBT.
            pub fn to_nbt(&self) -> io::Result<NbtValue> {
                let mut nbt = CompoundNbt::new(None);
                $(
                if let Some(value) = self.get::<$name>() {
                    let alias = Self::get_alias(&stringify!($name).to_string())
                    .ok_or(Error::new(ErrorKind::InvalidData, format!("Invalid alias(Component: {})", stringify!($name))))?;
                    nbt.insert(&alias, value.to_nbt()
                        .ok_or(Error::new(ErrorKind::InvalidData, format!("Invalid nbt value(Component: {})", stringify!($name))))?);
                }
                )*
                if let Some(value) = self.get::<Climate>() {
                    if let Some(value) = value.to_nbt() {
                        nbt.insert("minecraft:climate", value);
                    }
                }
                Ok(NbtValue::Compound(nbt))
            }
        }
    }
}
