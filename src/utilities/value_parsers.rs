//! Implement trait `clap::builder::TypedValueParser` for types.

use std::{ffi::OsStr, fs::File, io::Read as _, str::FromStr};

use ckb_sdk::{Address as CkbAddress, NetworkType};
use ckb_types::{bytes::Bytes, packed::OutPoint, prelude::*, H256};
use clap::{
    builder::{PathBufValueParser, PossibleValuesParser, StringValueParser, TypedValueParser},
    error::{ContextKind, ContextValue, ErrorKind},
};

#[derive(Clone, Debug)]
pub struct BinaryFileValueParser;

impl TypedValueParser for BinaryFileValueParser {
    type Value = Bytes;

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
        PathBufValueParser::new()
            .parse_ref(cmd, arg, value)
            .and_then(|filename| {
                let mut data = vec![];
                File::open(&filename)
                    .and_then(|mut file| file.read_to_end(&mut data))
                    .map(|_| data)
                    .map_err(|io_err| {
                        let msg = format!(
                            "failed to read file \"{}\" since {io_err}",
                            filename.display()
                        );
                        err.insert(ContextKind::InvalidValue, ContextValue::String(msg));
                        err
                    })
                    .map(Bytes::from)
            })
    }
}

#[derive(Clone, Debug)]
pub struct PrefixedHexStringValueParser;

impl TypedValueParser for PrefixedHexStringValueParser {
    type Value = String;

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
        let hex_str = StringValueParser::new().parse_ref(cmd, arg, value)?;
        if !hex_str.starts_with("0x") {
            err.insert(ContextKind::InvalidValue, ContextValue::String(hex_str));
            return Err(err);
        }
        Ok(hex_str)
    }
}

#[derive(Clone, Debug)]
pub struct U8VecValueParser;

impl TypedValueParser for U8VecValueParser {
    type Value = Vec<u8>;

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
        let hex_str = PrefixedHexStringValueParser.parse_ref(cmd, arg, value)?;
        let hex_bytes = hex_str[2..].as_bytes();
        let mut decoded = vec![0u8; hex_bytes.len() >> 1];
        faster_hex::hex_decode(hex_bytes, &mut decoded)
            .map_err(|_raw_err| {
                err.insert(ContextKind::InvalidValue, ContextValue::String(hex_str));
                err
            })
            .map(|_| decoded)
    }
}

#[derive(Clone, Debug)]
pub struct H256ValueParser;

impl TypedValueParser for H256ValueParser {
    type Value = H256;

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
        let hex_str = PrefixedHexStringValueParser.parse_ref(cmd, arg, value)?;
        Self::Value::from_str(&hex_str[2..]).map_err(|_raw_err| {
            err.insert(ContextKind::InvalidValue, ContextValue::String(hex_str));
            err
        })
    }
}

#[derive(Clone, Debug)]
pub struct OutPointValueParser;

impl TypedValueParser for OutPointValueParser {
    type Value = OutPoint;

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
        let vec = U8VecValueParser.parse_ref(cmd, arg, value)?;
        Self::Value::from_slice(&vec).map_err(|_raw_err| {
            let hex_str = faster_hex::hex_string(&vec);
            err.insert(
                ContextKind::InvalidValue,
                ContextValue::String(format!("0x{hex_str}")),
            );
            err
        })
    }
}

#[derive(Clone, Debug)]
pub struct NetworkTypeValueParser;

impl TypedValueParser for NetworkTypeValueParser {
    type Value = NetworkType;

    fn parse_ref(
        &self,
        cmd: &clap::Command,
        arg: Option<&clap::Arg>,
        value: &OsStr,
    ) -> Result<Self::Value, clap::Error> {
        PossibleValuesParser::new(["mainnet", "testnet"])
            .parse_ref(cmd, arg, value)
            .map(|s| match s.as_str() {
                "mainnet" => Self::Value::Mainnet,
                "testnet" => Self::Value::Testnet,
                _ => unreachable!(),
            })
    }
}

#[derive(Clone, Debug)]
pub struct AddressValueParser;

impl TypedValueParser for AddressValueParser {
    type Value = CkbAddress;

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
        let addr_str = StringValueParser::new().parse_ref(cmd, arg, value)?;
        Self::Value::from_str(&addr_str).map_err(|_raw_err| {
            err.insert(ContextKind::InvalidValue, ContextValue::String(addr_str));
            err
        })
    }
}
