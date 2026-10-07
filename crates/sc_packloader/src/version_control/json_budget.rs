use std::io::{self, Read};

pub const MAX_DEPTH: usize = 128;
pub const MAX_STRING_BYTES: usize = 1 << 20;
pub const MAX_CONTAINER_ITEMS: usize = 65_536;

pub struct BudgetReader<R> {
    inner: R,
    depth: usize,
    string_bytes: usize,
    in_string: bool,
    escaped: bool,
    containers: Vec<usize>,
}

impl<R> BudgetReader<R> {
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
