//! @risk none
//!
//! Bounded FIFO buffer for log collection

#[allow(dead_code)]
const MAX_BYTES: usize = 2048;

#[allow(dead_code)]
#[derive(Debug)]
pub(super) struct BoundedFifoVec {
    entries: Vec<String>,
    total_bytes: usize,
}

#[allow(dead_code)]
impl BoundedFifoVec {
    pub(super) fn new() -> Self {
        Self { entries: Vec::new(), total_bytes: 0 }
    }

    pub(super) fn push(&mut self, entry: String) {
        let entry_bytes = entry.len();
        self.total_bytes += entry_bytes;
        self.entries.push(entry);

        while self.total_bytes > MAX_BYTES && !self.entries.is_empty() {
            let removed = self.entries.remove(0);
            self.total_bytes -= removed.len();
        }
    }

    pub(super) fn to_vec(&self) -> Vec<String> {
        self.entries.clone()
    }

    pub(super) fn into_vec(self) -> Vec<String> {
        self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_limit() {
        let mut buf = BoundedFifoVec::new();
        buf.push("line1\n".to_string());
        buf.push("line2\n".to_string());
        assert_eq!(buf.into_vec(), vec!["line1\n", "line2\n"]);
    }

    #[test]
    fn exceeds_limit() {
        let mut buf = BoundedFifoVec::new();
        let large = "x".repeat(1500);
        buf.push(format!("{large}\n"));
        buf.push(format!("{large}\n"));
        let result = buf.into_vec();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn drops_oldest() {
        let mut buf = BoundedFifoVec::new();
        for i in 0..300 {
            buf.push(format!("line{i}\n"));
        }
        let result = buf.into_vec();
        assert!(!result.is_empty());
        assert!(result.len() < 300);
    }
}
