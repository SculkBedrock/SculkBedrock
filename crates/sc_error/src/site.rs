use std::panic::Location;

/// 错误的构造位置。
///
/// Construction site of an error.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct ScSite {
    /// Source file where the error was constructed.
    pub file: &'static str,
    /// Line number where the error was constructed.
    pub line: u32,
    /// Module path where the error was constructed.
    pub module: &'static str,
}

impl std::fmt::Display for ScSite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{} {}", self.file, self.line, self.module)
    }
}

/// 捕获调用者位置。给构造函数加 `#[track_caller]`
///
/// Capture the caller's site. Mark constructors with `#[track_caller]`
/// so the reported location is the construction point, not this helper.
#[track_caller]
pub fn capture_site() -> ScSite {
    let location = Location::caller();
    ScSite {
        file: location.file(),
        line: location.line(),
        module: module_path!(),
    }
}
