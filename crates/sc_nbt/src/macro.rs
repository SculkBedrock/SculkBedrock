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
