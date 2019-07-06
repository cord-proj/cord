use crate::errors::*;
use bytes::{buf::BufMut, BytesMut};
use error_chain::bail;
use pattern_matcher::Pattern;
use tokio_codec::{Decoder, Encoder};

use std::{
    convert::TryInto,
    hash::{Hash, Hasher},
    mem,
    result::Result as StdResult,
    u16, u32, u8,
};

macro_rules! read_int_frame {
    ($src:expr, $assign_to:expr, $type:ty) => {
        if $assign_to.is_none() {
            let len = mem::size_of::<$type>();

            // Check we have adequate data in the buffer before proceeding
            if $src.len() < len {
                return Ok(None);
            }

            $assign_to = Some(<$type>::from_be_bytes(
                (*$src.split_to(len)).try_into().unwrap(),
            ));
        }
    };
}

macro_rules! read_str_frame {
    ($src:expr, $assign_to:expr, $len:expr) => {
        if $assign_to.is_none() {
            // Check we have adequate data in the buffer before proceeding
            if $src.len() < $len {
                return Ok(None);
            }

            $assign_to = Some(String::from_utf8_lossy(&$src.split_to($len)).into_owned());
        }
    };
}

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

#[derive(Clone, Debug, Eq)]
pub enum Message {
    Provide(Pattern),
    Revoke(Pattern),
    Subscribe(Pattern),
    Unsubscribe(Pattern),
    Event(Pattern, String),
}

impl Message {
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

    unwrap_msg!(Provide, unwrap_provide);
    unwrap_msg!(Revoke, unwrap_revoke);
    unwrap_msg!(Subscribe, unwrap_subscribe);
    unwrap_msg!(Unsubscribe, unwrap_unsubscribe);

    pub fn namespace(&self) -> &Pattern {
        match self {
            Message::Provide(p) => p,
            Message::Revoke(p) => p,
            Message::Subscribe(p) => p,
            Message::Unsubscribe(p) => p,
            Message::Event(p, _) => p,
        }
    }

    // Unfortunately Rust doesn't provide a way to access the underlying
    // discriminant value. Thus we have to invent our own. Lame!
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
}

// Messages are only required to match on their Pattern. `Message::Event`
// contains a string argument, though we can safely ignore it, which requires
// a custom implementation of `PartialEq`.
impl PartialEq for Message {
    fn eq(&self, other: &Self) -> bool {
        match self {
            Message::Event(pattern, _) => match other {
                Message::Event(pattern1, _) => pattern == pattern1,
                _ => false,
            },
            _ => self.namespace() == other.namespace(),
        }
    }
}

impl Hash for Message {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Message::Provide(p) => p.hash(state),
            Message::Revoke(p) => p.hash(state),
            Message::Subscribe(p) => p.hash(state),
            Message::Unsubscribe(p) => p.hash(state),
            Message::Event(p, s) => {
                p.hash(state);
                s.hash(state);
            }
        }
    }
}

#[derive(Debug)]
pub struct MessageCodec {
    discriminant: Option<u8>,
    ns_length: Option<u16>,
    namespace: Option<String>,
    data_length: Option<u32>,
    data: Option<String>,
}

impl MessageCodec {
    pub fn new() -> Self {
        Self {
            discriminant: None,
            ns_length: None,
            namespace: None,
            data_length: None,
            data: None,
        }
    }
}

// Message framing on the wire looks like:
//      [u8             ][u16      ][bytestr  ][u32        ][bytestr]
//      [ns_discriminant][ns_length][namespace][data_length][data   ]
impl Encoder for MessageCodec {
    type Item = Message;
    type Error = Error;

    fn encode(&mut self, message: Self::Item, dst: &mut BytesMut) -> StdResult<(), Self::Error> {
        // Ensure the namespace will fit into a u16 buffer
        if message.namespace().len() > u16::MAX as usize {
            bail!(ErrorKind::OversizedNamespace);
        }

        // Write the message type to buffer
        dst.put_u8(message.poor_mans_discriminant());

        // Write namespace bytes to buffer
        dst.put_u16_be(message.namespace().len() as u16);
        dst.extend_from_slice(message.namespace().as_bytes());

        if let Message::Event(_, data) = message {
            // Ensure the message data will fit into a u32 buffer
            if data.len() > u32::MAX as usize {
                bail!(ErrorKind::OversizedData);
            }

            // Write data bytes to buffer
            dst.put_u32_be(data.len() as u32);
            dst.extend_from_slice(data.as_bytes());
        }

        Ok(())
    }
}

impl Decoder for MessageCodec {
    type Item = Message;
    type Error = Error;

    fn decode(&mut self, src: &mut BytesMut) -> StdResult<Option<Self::Item>, Self::Error> {
        // Read the discriminant (the type of message we're receiving)
        read_int_frame!(src, self.discriminant, u8);

        // Read the namespace's length
        read_int_frame!(src, self.ns_length, u16);

        // Read the namespace
        // If the namespace contains non-UTF8 bytes, replace them with
        // U+FFFD REPLACEMENT CHARACTER. This allows the decoding to continue despite the
        // bad data. In future it may be better to reject non-UTF8 encoded messages
        // entirely, but will require returning Option<Message> or similar to avoid
        // terminating the stream altogether by returning an error.
        read_str_frame!(
            src,
            self.namespace,
            *self.ns_length.as_ref().unwrap() as usize
        );

        // The magic number "4" represents the discriminant value for Message::Event. If
        // we are receiving a Message::Event, there is an extra data component to read.
        if *self.discriminant.as_ref().unwrap() == 4 {
            // Read the data's length
            read_int_frame!(src, self.data_length, u32);

            // Read the data
            read_str_frame!(src, self.data, *self.data_length.as_ref().unwrap() as usize);
        }

        // Reset these values in preparation for the next message
        self.ns_length = None;
        self.data_length = None;

        Ok(Some(Message::from_poor_mans_discriminant(
            self.discriminant.take().unwrap(),
            self.namespace.take().unwrap().into(),
            self.data.take(),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    #[test]
    fn test_encode_nodata_ok() {
        let msg = Message::Provide("/my/namespace".into());
        let mut bytes = BytesMut::new();
        let mut encoder = MessageCodec::new();
        encoder
            .encode(msg, &mut bytes)
            .expect("Failed to encode message");
        assert_eq!(bytes, Bytes::from("\0\0\r/my/namespace"));
    }

    #[test]
    fn test_encode_event_ok() {
        let msg = Message::Event("/my/namespace".into(), "abc, easy as 123".into());
        let mut bytes = BytesMut::new();
        let mut encoder = MessageCodec::new();
        encoder
            .encode(msg, &mut bytes)
            .expect("Failed to encode message");
        assert_eq!(
            bytes,
            Bytes::from("\x04\0\r/my/namespace\0\0\0\x10abc, easy as 123")
        );
    }

    #[test]
    fn test_encode_oversized_namespace() {
        #[allow(clippy::cast_lossless)]
        let long_str = String::from_utf8(vec![0; (u16::MAX as u32 + 1) as usize]).unwrap();
        let msg = Message::Unsubscribe(long_str.into());
        let mut bytes = BytesMut::new();
        let mut encoder = MessageCodec::new();
        match encoder
            .encode(msg, &mut bytes)
            .err()
            .expect("Test passed unexpectedly")
            .kind()
        {
            ErrorKind::OversizedNamespace => (),
            _ => panic!("Test passed unexpectedly"),
        }
    }

    #[test]
    fn test_encode_oversized_data() {
        // XXX Creating a String this large is very very very slow! In future this should
        // be mocked somehow.
        #[allow(clippy::cast_lossless)]
        let long_str = String::from_utf8(vec![0; (u32::MAX as u64 + 1) as usize]).unwrap();
        let msg = Message::Event("/".into(), long_str);
        let mut bytes = BytesMut::new();
        let mut encoder = MessageCodec::new();
        match encoder
            .encode(msg, &mut bytes)
            .err()
            .expect("Test passed unexpectedly")
            .kind()
        {
            ErrorKind::OversizedData => (),
            _ => panic!("Test passed unexpectedly"),
        }
    }

    #[test]
    fn test_decode_ok() {
        let mut bytes = BytesMut::from("\x01\0\r/my/namespace");
        let mut decoder = MessageCodec::new();
        let msg = decoder
            .decode(&mut bytes)
            .expect("Failed to decode message");
        assert_eq!(msg, Some(Message::Revoke("/my/namespace".into())));
    }

    #[test]
    fn test_decode_partial() {
        let mut bytes = BytesMut::new();
        let mut decoder = MessageCodec::new();

        // Test decoding nothing
        dbg!("Decode nada");
        let response = decoder
            .decode(&mut bytes)
            .expect("Failed to decode message");
        assert!(response.is_none());

        // Test decoding the discriminant
        dbg!("Decode discriminant");
        bytes.put_u8(Message::Event("/".into(), String::new()).poor_mans_discriminant());
        let response = decoder
            .decode(&mut bytes)
            .expect("Failed to decode message");
        assert!(response.is_none());

        // Test decoding partial namespace
        dbg!("Decode partial name");
        bytes.put_u16_be(13);
        bytes.extend_from_slice(b"/my/name");
        let response = decoder
            .decode(&mut bytes)
            .expect("Failed to decode message");
        assert!(response.is_none());

        // Test decoding the rest of the namespace
        dbg!("Decode name");
        bytes.extend_from_slice(b"space");
        let response = decoder
            .decode(&mut bytes)
            .expect("Failed to decode message");
        assert!(response.is_none());

        // Test decoding partial data
        dbg!("Decode partial data");
        bytes.put_u32_be(5);
        bytes.extend_from_slice(b"a");
        let response = decoder
            .decode(&mut bytes)
            .expect("Failed to decode message");
        assert!(response.is_none());

        // Test decoding the rest of the data
        dbg!("Decode data");
        bytes.extend_from_slice(b"bcde");
        let msg = decoder
            .decode(&mut bytes)
            .expect("Failed to decode message");
        assert_eq!(
            msg,
            Some(Message::Event("/my/namespace".into(), "abcde".into()))
        );
    }

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
}
