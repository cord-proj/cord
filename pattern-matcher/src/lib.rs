use std::{cmp::Ordering, convert::From, ops::Deref};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Pattern(String);

impl Pattern {
    pub fn new<S: Into<String>>(pattern: S) -> Self {
        Pattern(pattern.into())
    }
}

impl PartialOrd for Pattern {
    fn partial_cmp(&self, other: &Pattern) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Pattern {
    // The most generic namespace has the greatest value
    fn cmp(&self, other: &Pattern) -> Ordering {
        if self == other {
            Ordering::Equal
        }
        // E.g. /a > /a/b
        else if other.0.starts_with(&self.0) && other.0.chars().nth(self.0.len()) == Some('/') {
            Ordering::Greater
        // E.g. /a/b < /a
        } else {
            Ordering::Less
        }
    }
}

impl Deref for Pattern {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S> From<S> for Pattern
where
    S: Into<String>,
{
    fn from(s: S) -> Pattern {
        Pattern::new(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ord_equal() {
        assert_eq!(Pattern::new("/a"), Pattern::new("/a"));
    }

    #[test]
    fn test_ord_unequal() {
        assert!(Pattern::new("/a/b") < Pattern::new("/ab"))
    }

    #[test]
    fn test_ord_greater() {
        assert!(Pattern::new("/a") > Pattern::new("/a/b"))
    }

    #[test]
    fn test_ord_less() {
        assert!(Pattern::new("/a/b") < Pattern::new("/a"))
    }
}
