//! Numbers as `model_select` prints them: the shortest text that reads back the same.
use super::*;

/// A number in scientific notation, without the noise of its last bits:
/// `1.2e-4`, not `1.1999999999999999e-4`
#[derive(Debug, Clone, Copy)]
pub struct Short(pub f64);

impl fmt::Display for Short {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let x = self.0;
        // the shortest exact form, if short; else the fewest digits that
        // give back `x` within 1e-12 (relative)
        let mut text = NumberText::default();
        write!(text, "{x:e}")?;
        if text.len <= 12 {
            return f.write_str(text.as_str());
        }
        let in_digits = |digits: usize, text: &mut NumberText| {
            text.len = 0;
            write!(text, "{x:.digits$e}").is_ok()
                && text
                    .as_str()
                    .parse::<f64>()
                    .is_ok_and(|y| (y - x).abs() <= x.abs() * 1e-12)
        };
        // (one more digit can only come nearer to `x`, so this search can halve)
        if !in_digits(15, &mut text) {
            return write!(f, "{x:e}");
        }
        let (mut low, mut high) = (0, 15);
        while low < high {
            let middle = (low + high) / 2;
            if in_digits(middle, &mut text) {
                high = middle;
            } else {
                low = middle + 1;
            }
        }
        in_digits(high, &mut text);
        f.write_str(text.as_str())
    }
}

/// The text of a number, on the stack: one for each number of a listing
#[derive(Default)]
struct NumberText {
    bytes: [u8; 32],
    len: usize,
}

impl NumberText {
    fn as_str(&self) -> &str {
        // (`write_str` adds whole strings)
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or_default()
    }
}

impl fmt::Write for NumberText {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.len + s.len();
        self.bytes
            .get_mut(self.len..end)
            .ok_or(fmt::Error)?
            .copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}
