use std::{convert::From, ops::Deref};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Pattern(String);

impl Pattern {
    pub fn new<S: Into<String>>(pattern: S) -> Self {
        Pattern(pattern.into())
    }

    // The most generic namespace has the greatest value
    pub fn contains(&self, other: &Pattern) -> bool {
        if self == other {
            true
        } else {
            // E.g. /a contains /a/b
            other.0.starts_with(&self.0) && other.0.chars().nth(self.0.len()) == Some('/')
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
        assert!(!Pattern::new("/a/b").contains(&Pattern::new("/ab")))
    }

    #[test]
    fn test_ord_greater() {
        assert!(Pattern::new("/a").contains(&Pattern::new("/a/b")))
    }

    #[test]
    fn test_ord_less() {
        assert!(!Pattern::new("/a/b").contains(&Pattern::new("/a")))
    }
}
