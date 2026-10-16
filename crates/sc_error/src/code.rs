/// Stable machine-readable error code.
///
/// Codes are `&'static str` dotted paths such as `"world.storage.io"`.
/// They are append-only: never rename a code, only add new ones.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct ScCode(pub &'static str);

impl ScCode {
    /// Create a code from a static string.
    pub const fn new(code: &'static str) -> Self {
        Self(code)
    }

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
