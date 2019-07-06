use crate::errors::*;
use byteorder::{ByteOrder, NetworkEndian};
use bytes::{buf::BufMut, BytesMut};
use error_chain::bail;
use pattern_matcher::Pattern;
use tokio_codec::{Decoder, Encoder};

use std::{
    fmt::Write,
    hash::{Hash, Hasher},
    mem,
    result::Result as StdResult,
    u16, u32,
};

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
            _ => self == other,
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

pub struct MessageCodec {
    discriminant: Option<u8>,
    ns_length: Option<usize>,
    namespace: Option<String>,
    data_length: Option<usize>,
}

impl MessageCodec {
    pub fn new() -> Self {
        Self {
            discriminant: None,
            ns_length: None,
            namespace: None,
            data_length: None,
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
        if message.namespace().len() <= u16::MAX as usize {
            bail!(ErrorKind::OversizedNamespace);
        }

        // Write the message type to buffer
        dst.put_u8(message.poor_mans_discriminant());

        // Write namespace bytes to buffer
        dst.put_u16_be(message.namespace().len() as u16);
        dst.write_str(&message.namespace())
            .chain_err(|| ErrorKind::BufferFull)?;

        if let Message::Event(_, data) = message {
            // Ensure the message data will fit into a u32 buffer
            if data.len() <= u32::MAX as usize {
                bail!(ErrorKind::OversizedData);
            }

            // Write data bytes to buffer
            dst.put_u32_be(data.len() as u32);
            dst.write_str(&data).chain_err(|| ErrorKind::BufferFull)?;
        }

        Ok(())
    }
}

impl Decoder for MessageCodec {
    type Item = Message;
    type Error = Error;

    fn decode(&mut self, src: &mut BytesMut) -> StdResult<Option<Self::Item>, Self::Error> {
        // Check we have adequate data in the buffer before proceeding
        if src.len() < mem::size_of::<u8>() {
            return Ok(None);
        }

        // Read the discriminant (the type of message we're receiving)
        let discriminant = match self.discriminant {
            Some(d) => d,
            None => u8::from_be_bytes([src.split_to(mem::size_of::<u8>())[0]]),
        };

        // Check we have adequate data in the buffer before proceeding
        if src.len() < mem::size_of::<u16>() {
            // Store discriminant so we don't try and read it again
            self.discriminant = Some(discriminant);
            return Ok(None);
        }

        let ns_length = match self.ns_length {
            Some(l) => l,
            None => NetworkEndian::read_u16(&src.split_to(mem::size_of::<u16>())) as usize,
        };

        // If we don't have the full message yet, wait for the buffer to fill
        // up more.
        if src.len() < ns_length {
            // Store this length so that we don't try and read it again next time
            self.ns_length = Some(ns_length);
            return Ok(None);
        }

        // Read the namespace
        // If the namespace contains non-UTF8 bytes, replace them with
        // U+FFFD REPLACEMENT CHARACTER. This allows the decoding to continue despite the
        // bad data. In future it may be better to reject non-UTF8 encoded messages
        // entirely, but will require returning Option<Message> or similar to avoid
        // terminating the stream altogether by returning an error.
        let ns_bytes = src.split_to(ns_length);
        let namespace = String::from_utf8_lossy(&ns_bytes);

        // Check we have adequate data in the buffer before proceeding
        if src.len() < mem::size_of::<u32>() {
            // Store namespace so we don't try and read it again
            self.namespace = Some(namespace.into_owned());
            return Ok(None);
        }

        // The magic number "4" represents the discriminant value for
        // Message::Event. If we are receiving a Message::Event, there is an
        // extra data component to read.
        let data = if discriminant == 4 {
            let data_length = match self.data_length {
                Some(l) => l,
                None => NetworkEndian::read_u32(&src.split_to(mem::size_of::<u32>())) as usize,
            };

            // If we don't have the full message yet, wait for the buffer to fill
            // up more.
            if src.len() < data_length {
                // Store this length so that we don't try and read it again next time
                self.data_length = Some(data_length);
                return Ok(None);
            }

            // Read the message data
            let data_bytes = src.split_to(data_length);
            Some(String::from_utf8_lossy(&data_bytes).into_owned())
        } else {
            None
        };

        Ok(Some(Message::from_poor_mans_discriminant(
            discriminant,
            namespace.into(),
            data,
        )))
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use bytes::Bytes;
//
//     #[test]
//     fn test_message_new_ok() {
//         assert!(Message::new("abc").is_ok());
//     }
//
//     #[test]
//     fn test_message_new_oversize() {
//         let long_str = String::from_utf8(vec![0; 65536]).unwrap();
//         let e = Message::new(long_str).unwrap_err();
//         match e.kind() {
//             ErrorKind::OversizedNamespace => (),
//             _ => panic!("Expected variant ErrorKind::OversizedNamespace"),
//         }
//     }
//
//     #[test]
//     fn test_encode_ok() {
//         let msg = Message {
//             namespace: "/my/namespace".into(),
//         };
//         let mut bytes = BytesMut::new();
//         let mut encoder = MessageCodec::new();
//         encoder
//             .encode(msg, &mut bytes)
//             .expect("Failed to encode message");
//         assert_eq!(bytes, Bytes::from("\0\r/my/namespace"));
//     }
//
//     #[test]
//     #[should_panic(expected = "Namespace length cannot be greater than a u16")]
//     fn test_encode_oversize() {
//         // 65536 = u16::MAX + 1
//         // Hence a 32 bit int
//         let long_str = String::from_utf8(vec![0; 65536]).unwrap();
//         let msg = Message {
//             namespace: long_str,
//         };
//         let mut bytes = BytesMut::new();
//         let mut encoder = MessageCodec::new();
//         encoder
//             .encode(msg, &mut bytes)
//             .expect("Failed to encode message");
//     }
//
//     #[test]
//     fn test_decode() {
//         let mut bytes = BytesMut::from("\0\r/my/namespace");
//         let mut decoder = MessageCodec::new();
//         let msg = decoder
//             .decode(&mut bytes)
//             .expect("Failed to decode message");
//         assert_eq!(
//             msg,
//             Some(Message {
//                 namespace: "/my/namespace".into()
//             })
//         );
//     }
//
//     #[test]
//     fn test_decode_partial() {
//         let mut bytes = BytesMut::from("\0\r/my/name");
//         let mut decoder = MessageCodec::new();
//
//         // Test decoding a partial message
//         let response = decoder
//             .decode(&mut bytes)
//             .expect("Failed to decode message");
//         assert!(response.is_none());
//
//         // Test decoding the rest of the message
//         bytes
//             .write_str("space")
//             .expect("Failed to write bytes to buffer");
//         let msg = decoder
//             .decode(&mut bytes)
//             .expect("Failed to decode message");
//         assert_eq!(
//             msg,
//             Some(Message {
//                 namespace: "/my/namespace".into()
//             })
//         )
//     }
// }
