#[macro_export]
macro_rules! components_export {
    ($struct_name:ident, $($name:ident = $alias:expr,)*) => {
        // Define the primary struct
        #[allow(non_snake_case)]
        #[allow(unreachable_patterns)]
        #[allow(unused_variables)]
        #[derive(Deserialize, Clone)]
        #[serde(untagged)]
        pub enum $struct_name {
            Map(Map<String, Value>)
        }
        impl std::fmt::Debug for $struct_name {
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
            pub fn new_single<T: serde::Serialize + sc_ecs::component::Component>(component: T) -> Option<Self> {
                let mut map = Map::new();
                let alias = Self::get_alias(&T::name())?;
                map.insert(alias, serde_json::to_value(component).ok()?);
                return Some(Self::Map(map));
            }

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


            pub fn get_alias(struct_name: &String) -> Option<String> {
                $(
                if struct_name == stringify!($name) {
                    return Some($alias.to_string());
                }
                )*
                None
            }

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

#[macro_export]
macro_rules! types_export {
    ($struct_name:ident, $enum_name:ident, $($name:ident = $alias:expr,)*) => {
        // Main struct definition.
        pub enum $enum_name {
            Empty,
            $(
                $name($name),
            )*
        }
        #[allow(non_snake_case)]
        #[derive(Deserialize, Debug)]
        pub struct $struct_name {
            pub format_version: String,
            $(
                #[serde(alias = $alias)]
                pub $name: Option<$name>,
            )*
        }
        impl $struct_name {
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

#[macro_export]
macro_rules! biome_nbt_export {
    ($struct_name:ident, $($name:ident),*) => {
        impl $struct_name {
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
