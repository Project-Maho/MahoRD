//! Pure codecs for the MahoRD version 3 wire protocol.
//!
//! This crate performs no I/O. It only converts validated protocol values to
//! and from their byte-exact wire representation and incrementally parses the
//! TCP length-prefix framing used by the transport layer.

mod codec;
mod control;
mod error;
mod framing;
mod handshake;
mod input;
mod media;
mod packet;
mod pairing;
mod relay;

pub use control::*;
pub use error::CodecError;
pub use framing::*;
pub use handshake::*;
pub use input::*;
pub use media::*;
pub use packet::*;
pub use pairing::*;
pub use relay::*;

/// A value with a complete version 3 wire encoding.
pub trait WireCodec: Sized {
    /// Encodes this value into its canonical wire representation.
    fn encode(&self) -> Result<Vec<u8>, CodecError>;

    /// Decodes one complete value from `input`.
    fn decode(input: &[u8]) -> Result<Self, CodecError>;
}
