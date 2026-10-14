#[derive(Debug, PartialEq)]
pub(crate) struct Hunk {
    pub(crate) old: u64,
    pub(crate) old_end: u64,
    pub(crate) new: u64,
    pub(crate) new_end: u64,
}

impl Hunk {
    pub(crate) fn parse(header: &str) -> Option<Self> {
        let header = header.strip_prefix("@@ -")?;
        let (old, rest) = header.split_once(" +")?;
        let (new, suffix) = rest.split_once(" @@")?;
        if !suffix.is_empty() && !suffix.starts_with(' ') {
            return None;
        }
        let (old, old_end) = range(old)?;
        let (new, new_end) = range(new)?;
        if old == old_end && new == new_end {
            return None;
        }
        Some(Self {
            old,
            old_end,
            new,
            new_end,
        })
    }

    pub(crate) fn complete(&self) -> bool {
        self.old == self.old_end && self.new == self.new_end
    }
}

fn range(value: &str) -> Option<(u64, u64)> {
    let (start, count) = value.split_once(',').unwrap_or((value, "1"));
    if ![start, count]
        .iter()
        .all(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    let start = u64::from(start.parse::<u32>().ok()?);
    let count = u64::from(count.parse::<u32>().ok()?);
    let end = start + count;
    if start == 0 && count != 0 {
        return None;
    }
    if end > u64::from(u32::MAX) + 1 {
        return None;
    }
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_hunk_header_syntax() {
        assert_eq!(Hunk::parse("@@ invalid @@"), None);
    }

    #[test]
    fn rejects_hunks_with_two_empty_ranges() {
        assert_eq!(Hunk::parse("@@ -0,0 +1,0 @@"), None);
    }

    #[test]
    fn rejects_nonempty_hunk_ranges_starting_at_zero() {
        assert_eq!(range("0"), None);
    }

    #[test]
    fn rejects_hunk_range_starts_exceeding_u32_maximum() {
        assert_eq!(range("4294967296"), None);
    }
}
