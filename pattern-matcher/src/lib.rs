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
            other.0.starts_with(&self.0)
                && (self.0.len() == 1 || other.0.chars().nth(self.0.len()) == Some('/'))
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
    fn test_root_namespace() {
        assert!(Pattern::new("/").contains(&Pattern::new("/a")));
    }

    #[test]
    fn test_equality() {
        assert_eq!(Pattern::new("/a"), Pattern::new("/a"));
    }

    #[test]
    fn test_case_inequality() {
        assert_ne!(Pattern::new("/a"), Pattern::new("/A"));
    }

    #[test]
    fn test_inequality() {
        assert!(!Pattern::new("/a/b").contains(&Pattern::new("/ab")));
    }

    #[test]
    fn test_contains_child_namespace() {
        assert!(Pattern::new("/a").contains(&Pattern::new("/a/b")));
    }

    #[test]
    fn test_not_contains_parent() {
        assert!(!Pattern::new("/a/b").contains(&Pattern::new("/a")));
    }
}
