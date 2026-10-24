#[macro_export]
macro_rules! write_impl {
    ($($alias:ident: $name:ty),*) => {
        $(
        impl NbtCustomWrite for $name {
            fn write<T: NbtWriteTrait>(&self, writer: &mut NbtWriter) -> io::Result<()> {
                writer.write::<T>(&self.to_nbt().ok_or(io::Error::new(io::ErrorKind::Other, format!("Failed to write {}", stringify!($alias))))?)
            }

            fn to_nbt(&self) -> Option<NbtValue> {
                Some(NbtValue::$alias(self.clone()))
            }
        }
        )*
    };
}

#[macro_export]
macro_rules! item_nbt {
    ($($key:expr => $val:tt),* $(,)?) => {{
        let mut compound = $crate::CompoundNbt::new(None);
        $(
            compound.insert($key, $crate::nbt_val!($val));
        )*
        compound
    }};
    ($($key:ident : $val:tt),* $(,)?) => {{
        let mut compound = $crate::CompoundNbt::new(None);
        $(
            compound.insert(stringify!($key), $crate::nbt_val!($val));
        )*
        compound
    }};
    () => {
        $crate::CompoundNbt::new(None)
    };
}

#[macro_export]
macro_rules! nbt_val {
    ({ $($key:expr => $val:tt),* $(,)? }) => {
        $crate::NbtValue::Compound($crate::item_nbt! { $($key => $val),* })
    };
    ({ $($key:ident : $val:tt),* $(,)? }) => {
        $crate::NbtValue::Compound($crate::item_nbt! { $($key : $val),* })
    };
    ([ $($elem:tt),* $(,)? ]) => {
        $crate::NbtValue::List(vec![ $( $crate::nbt_val!($elem) ),* ])
    };
    ($val:expr) => {
        $crate::NbtValue::from($val)
    };
}
