//! 包 JSON 读取的内存预算保护：限制嵌套深度、字符串长度与容器条目数。
//!
//! Memory-budget guard for pack JSON reading: caps nesting depth, string
//! length, and container item counts.

use std::io::{self, Read};

/// JSON 最大嵌套深度。
///
/// Maximum JSON nesting depth.
pub const MAX_DEPTH: usize = 128;
/// 单个 JSON 字符串最大字节数（1 MiB）。
///
/// Maximum bytes for a single JSON string (1 MiB).
pub const MAX_STRING_BYTES: usize = 1 << 20;
/// 单个 JSON 容器最大条目数。
///
/// Maximum item count for a single JSON container.
pub const MAX_CONTAINER_ITEMS: usize = 65_536;

/// 预算读取器：透传读取的同时检查 JSON 预算，超限返回 `InvalidData` 错误。
///
/// Budget reader: transparently reads while checking JSON budgets; returns
/// an `InvalidData` error when a budget is exceeded.
pub struct BudgetReader<R> {
    /// 被包裹的读取器。
    ///
    /// The wrapped reader.
    inner: R,
    /// 当前嵌套深度。
    ///
    /// Current nesting depth.
    depth: usize,
    /// 当前字符串已累计字节数。
    ///
    /// Bytes accumulated for the current string.
    string_bytes: usize,
    /// 是否处于字符串字面量内。
    ///
    /// Whether inside a string literal.
    in_string: bool,
    /// 下一个字符是否为转义字符。
    ///
    /// Whether the next character is escaped.
    escaped: bool,
    /// 各层容器已见条目数栈。
    ///
    /// Stack of item counts seen per container level.
    containers: Vec<usize>,
}

impl<R> BudgetReader<R> {
    /// 用初始状态包裹读取器。
    ///
    /// Wraps a reader with initial state.
    ///
    /// - `inner`: 被包裹的读取器
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            depth: 0,
            string_bytes: 0,
            in_string: false,
            escaped: false,
            containers: Vec::new(),
        }
    }

    /// 逐字节检查预算：字符串超长、嵌套过深、容器过大均报错。
    ///
    /// Checks budgets byte by byte: over-long strings, too-deep nesting, and
    /// oversized containers all error.
    fn inspect(&mut self, bytes: &[u8]) -> io::Result<()> {
        for &byte in bytes {
            if self.in_string {
                if self.escaped {
                    self.escaped = false;
                    continue;
                }
                if byte == b'\\' {
                    self.escaped = true;
                } else if byte == b'"' {
                    self.in_string = false;
                    self.string_bytes = 0;
                } else {
                    self.string_bytes = self.string_bytes.saturating_add(1);
                    if self.string_bytes > MAX_STRING_BYTES {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "JSON string exceeds memory budget",
                        ));
                    }
                }
                continue;
            }
            match byte {
                b'"' => {
                    self.in_string = true;
                    self.string_bytes = 0;
                }
                b'{' | b'[' => {
                    self.depth += 1;
                    if self.depth > MAX_DEPTH {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "JSON nesting exceeds memory budget",
                        ));
                    }
                    self.containers.push(0);
                }
                b'}' | b']' => {
                    self.depth = self.depth.saturating_sub(1);
                    self.containers.pop();
                }
                b',' => {
                    if let Some(items) = self.containers.last_mut() {
                        *items += 1;
                        if *items >= MAX_CONTAINER_ITEMS {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "JSON container exceeds memory budget",
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

impl<R: Read> Read for BudgetReader<R> {
    /// 读取一块数据并对其执行预算检查。
    ///
    /// Reads one chunk of data and runs the budget check over it.
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.inspect(&buffer[..read])?;
        Ok(read)
    }
}

#[cfg(test)]
mod tests {
    use super::{BudgetReader, MAX_CONTAINER_ITEMS, MAX_DEPTH, MAX_STRING_BYTES};
    use std::io::Read;

    #[test]
    fn rejects_deep_json() {
        let input = format!(
            "{}0{}",
            "[".repeat(MAX_DEPTH + 1),
            "]".repeat(MAX_DEPTH + 1)
        );
        let mut reader = BudgetReader::new(input.as_bytes());
        assert!(reader.read_to_end(&mut Vec::new()).is_err());
    }

    #[test]
    fn rejects_long_string() {
        let input = format!("\"{}\"", "x".repeat(MAX_STRING_BYTES + 1));
        let mut reader = BudgetReader::new(input.as_bytes());
        assert!(reader.read_to_end(&mut Vec::new()).is_err());
    }

    #[test]
    fn rejects_large_container() {
        let input = format!(
            "[{}]",
            (0..=MAX_CONTAINER_ITEMS)
                .map(|_| "0")
                .collect::<Vec<_>>()
                .join(",")
        );
        let mut reader = BudgetReader::new(input.as_bytes());
        assert!(reader.read_to_end(&mut Vec::new()).is_err());
    }
}
