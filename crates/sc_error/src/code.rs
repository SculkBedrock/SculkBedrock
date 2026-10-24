/// 稳定的机器可读错误码。
///
/// 错误码为点分路径形式的 `&'static str`，例如 `"world.storage.io"`。
/// 只增不改：绝不重命名已有错误码，只能新增。
///
/// Stable machine-readable error code.
///
/// Codes are `&'static str` dotted paths such as `"world.storage.io"`.
/// They are append-only: never rename a code, only add new ones.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct ScCode(pub &'static str);

impl ScCode {
    /// 由静态字符串创建错误码。
    ///
    /// Create a code from a static string.
    pub const fn new(code: &'static str) -> Self {
        Self(code)
    }

    /// 原始码字符串。
    ///
    /// The raw code string.
    pub fn as_str(self) -> &'static str {
        self.0
    }
}

impl std::fmt::Display for ScCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
