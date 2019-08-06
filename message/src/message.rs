use pattern_matcher::Pattern;

use std::mem;

macro_rules! unwrap_msg {
    ($variant:tt, $func_name:ident) => (
        pub fn $func_name(self) -> Pattern {
            match self {
                Message::$variant(pattern) => pattern,
                _ => panic!("Expected Message variant $variant, got {:?}", self)
            }
        }
    )
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Message {
    Provide(Pattern),
    Revoke(Pattern),
    Subscribe(Pattern),
    Unsubscribe(Pattern),
    Event(Pattern, String),
}

impl Message {
    // Check that the discriminant is in range. The magic number "4" is the maximum
    // integer assigned to a Message variant. See `poor_mans_discriminant()` for details.
    // Note that we take a ref u8 instead of an owned u8 to be compatible with
    // `Option::filter`, which passes all args by ref.
    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub fn test_poor_mans_discriminant(discriminant: &u8) -> bool {
        *discriminant <= 4
    }

    // This function takes a discriminant and all other data necessary to instantiate a
    // new Message variant. To avoid panics, first check the discriminant with
    // `test_poor_mans_discriminant()`.
    pub fn from_poor_mans_discriminant(
        discriminant: u8,
        namespace: Pattern,
        data: Option<String>,
    ) -> Self {
        match discriminant {
            0 => Message::Provide(namespace),
            1 => Message::Revoke(namespace),
            2 => Message::Subscribe(namespace),
            3 => Message::Unsubscribe(namespace),
            4 => Message::Event(
                namespace,
                data.expect("Data must be present for Message::Event type"),
            ),
            _ => panic!("Invalid discriminant {}", discriminant),
        }
    }

    // These macro-generated functions will destructure the Message enum, returning an
    // owned Pattern.
    unwrap_msg!(Provide, unwrap_provide);
    unwrap_msg!(Revoke, unwrap_revoke);
    unwrap_msg!(Subscribe, unwrap_subscribe);
    unwrap_msg!(Unsubscribe, unwrap_unsubscribe);

    // Return the namespace pattern for any variant
    pub fn namespace(&self) -> &Pattern {
        match self {
            Message::Provide(p) => p,
            Message::Revoke(p) => p,
            Message::Subscribe(p) => p,
            Message::Unsubscribe(p) => p,
            Message::Event(p, _) => p,
        }
    }

    // XXX Unfortunately Rust doesn't provide a way to access the underlying discriminant
    // value. Thus we have to invent our own. Lame!
    //
    // The fn name is going to stay horrible until a better solution is found. Don't want
    // to get comfortable with this hack :)
    // https://github.com/rust-lang/rust/issues/34244
    pub fn poor_mans_discriminant(&self) -> u8 {
        match self {
            Message::Provide(_) => 0,
            Message::Revoke(_) => 1,
            Message::Subscribe(_) => 2,
            Message::Unsubscribe(_) => 3,
            Message::Event(_, _) => 4,
        }
    }

    pub fn contains(&self, other: &Message) -> bool {
        mem::discriminant(self) == mem::discriminant(&other)
            && self.namespace().contains(other.namespace())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_poor_mans_discriminant() {
        let pattern = Pattern::new("/");

        let provide = Message::Provide(pattern.clone());
        assert_eq!(
            Message::from_poor_mans_discriminant(
                provide.poor_mans_discriminant(),
                pattern.clone(),
                None
            ),
            provide
        );

        let revoke = Message::Revoke(pattern.clone());
        assert_eq!(
            Message::from_poor_mans_discriminant(
                revoke.poor_mans_discriminant(),
                pattern.clone(),
                None
            ),
            revoke
        );

        let subscribe = Message::Subscribe(pattern.clone());
        assert_eq!(
            Message::from_poor_mans_discriminant(
                subscribe.poor_mans_discriminant(),
                pattern.clone(),
                None
            ),
            subscribe
        );

        let unsubscribe = Message::Unsubscribe(pattern.clone());
        assert_eq!(
            Message::from_poor_mans_discriminant(
                unsubscribe.poor_mans_discriminant(),
                pattern.clone(),
                None
            ),
            unsubscribe
        );

        let event = Message::Event(pattern.clone(), String::new());
        assert_eq!(
            Message::from_poor_mans_discriminant(
                event.poor_mans_discriminant(),
                pattern.clone(),
                Some(String::new())
            ),
            event
        );
    }

    #[test]
    fn test_message_contains() {
        let message = Message::Provide("/a".into());

        // Should contain the same namespace
        assert!(message.contains(&Message::Provide("/a".into())));
        // Should contain a sub-namespace
        assert!(message.contains(&Message::Provide("/a/b".into())));
        // Should not contain a different message type
        assert!(!message.contains(&Message::Revoke("/a".into())));
        // Should not contain a different namespace
        assert!(!message.contains(&Message::Provide("/c".into())));

        // Event messages should not consider their data components
        assert!(Message::Event("/a".into(), String::new())
            .contains(&Message::Event("/a".into(), "b".into())));

        // Root namespace should match everything
        assert!(Message::Provide("/".into()).contains(&Message::Provide("/a/b/c".into())));
    }
}
