//! Structs for sensitive data.

use std::{ffi::OsStr, fmt};

use clap::{
    builder::{TypedValueParser, ValueParserFactory},
    error::{ContextKind, ContextValue, ErrorKind},
};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::utilities::value_parsers;

/// A 256 bits bytes used for sensitive data, such as private keys.
/// It's implemented a `Drop` handler which erase its memory when it dropped.
/// This ensures that sensitive data is securely erased from memory when it is
/// no longer needed.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct Key256Bits([u8; 32]);

impl fmt::Debug for Key256Bits {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Key256Bits")
    }
}

impl fmt::Display for Key256Bits {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "256-bits-key")
    }
}

impl AsRef<[u8; 32]> for Key256Bits {
    fn as_ref(&self) -> &[u8; 32] {
        &self.0
    }
}

impl ValueParserFactory for Key256Bits {
    type Parser = Key256BitsValueParser;

    fn value_parser() -> Self::Parser {
        Key256BitsValueParser
    }
}

#[derive(Clone, Debug)]
pub struct Key256BitsValueParser;

impl TypedValueParser for Key256BitsValueParser {
    type Value = Key256Bits;

    fn parse_ref(
        &self,
        cmd: &clap::Command,
        arg: Option<&clap::Arg>,
        value: &OsStr,
    ) -> Result<Self::Value, clap::Error> {
        let mut err = clap::Error::new(ErrorKind::InvalidValue).with_cmd(cmd);
        if let Some(arg) = arg {
            err.insert(
                ContextKind::InvalidArg,
                ContextValue::String(arg.to_string()),
            );
        }
        let data = value_parsers::BinaryFileValueParser.parse_ref(cmd, arg, value)?;
        if data.len() != 32 {
            let msg = format!("the input 256 bits key file contains {} bytes", data.len());
            err.insert(ContextKind::InvalidValue, ContextValue::String(msg));
            return Err(err);
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&data);
        Ok(Key256Bits(arr))
    }
}
