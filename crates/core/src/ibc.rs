//! IBC-related data types

pub mod event;

use std::str::FromStr;

use borsh::{BorshDeserialize, BorshSchema, BorshSerialize};
use borsh_ext::BorshSerializeExt;
use data_encoding::{DecodePartial, HEXLOWER, HEXLOWER_PERMISSIVE, HEXUPPER};
pub use ibc::*;
use namada_macros::BorshDeserializer;
#[cfg(feature = "migrations")]
use namada_migrations::*;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use self::event::IbcEvent;
use super::address::HASH_LEN;
use crate::event::extend::{ReadFromEventAttributes, Success as SuccessAttr};
use crate::event::EventError;
use crate::ibc::apps::transfer::types::msgs::transfer::MsgTransfer;
use crate::ibc::apps::transfer::types::{Memo, PrefixedDenom, TracePath};
use crate::ibc::core::handler::types::events::Error as IbcEventError;
use crate::ibc::primitives::proto::Protobuf;
use crate::token::Transfer;

/// The event type defined in ibc-rs for receiving a token
pub const EVENT_TYPE_PACKET: &str = "fungible_token_packet";
/// The event type defined in ibc-rs for IBC denom
pub const EVENT_TYPE_DENOM_TRACE: &str = "denomination_trace";

/// IBC token hash derived from a denomination.
#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
    BorshSerialize,
    BorshDeserialize,
    BorshDeserializer,
    BorshSchema,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
)]
#[repr(transparent)]
pub struct IbcTokenHash(pub [u8; HASH_LEN]);

impl std::fmt::Display for IbcTokenHash {
    #[inline(always)]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", HEXLOWER.encode(&self.0))
    }
}

impl FromStr for IbcTokenHash {
    type Err = DecodePartial;

    fn from_str(h: &str) -> Result<Self, Self::Err> {
        let mut output = [0u8; HASH_LEN];
        HEXLOWER_PERMISSIVE.decode_mut(h.as_ref(), &mut output)?;
        Ok(IbcTokenHash(output))
    }
}

/// IBC transfer message to send from a shielded address
#[derive(Debug, Clone)]
pub struct MsgShieldedTransfer {
    /// IBC transfer message
    pub message: MsgTransfer,
    /// MASP tx with token transfer
    pub shielded_transfer: IbcShieldedTransfer,
}

impl BorshSerialize for MsgShieldedTransfer {
    fn serialize<W: std::io::Write>(
        &self,
        writer: &mut W,
    ) -> std::io::Result<()> {
        let encoded_msg = self.message.clone().encode_vec();
        let members = (encoded_msg, self.shielded_transfer.clone());
        BorshSerialize::serialize(&members, writer)
    }
}

impl BorshDeserialize for MsgShieldedTransfer {
    fn deserialize_reader<R: std::io::Read>(
        reader: &mut R,
    ) -> std::io::Result<Self> {
        use std::io::{Error, ErrorKind};
        let (msg, shielded_transfer): (Vec<u8>, IbcShieldedTransfer) =
            BorshDeserialize::deserialize_reader(reader)?;
        let message = MsgTransfer::decode_vec(&msg)
            .map_err(|err| Error::new(ErrorKind::InvalidData, err))?;
        Ok(Self {
            message,
            shielded_transfer,
        })
    }
}

/// IBC shielded transfer
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, BorshDeserializer)]
pub struct IbcShieldedTransfer {
    /// The IBC event type
    pub transfer: Transfer,
    /// The attributes of the IBC event
    pub masp_tx: masp_primitives::transaction::Transaction,
}

impl std::fmt::Display for IbcShieldedTransfer {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", Memo::from(self))
    }
}

impl FromStr for IbcShieldedTransfer {
    type Err = Error;

    #[inline]
    fn from_str(s: &str) -> Result<Self, Error> {
        Memo::from(s.to_owned()).try_into()
    }
}

#[allow(missing_docs)]
#[derive(Error, Debug)]
pub enum Error {
    #[error("Event error: {0}")]
    Event(EventError),
    #[error("IBC event error: {0}")]
    IbcEvent(IbcEventError),
    #[error("IBC transfer memo HEX decoding error: {0}")]
    DecodingHex(data_encoding::DecodeError),
    #[error("IBC transfer memo decoding error: {0}")]
    DecodingShieldedTransfer(std::io::Error),
}

/// Returns the trace path and the token string if the denom is an IBC
/// denom.
pub fn is_ibc_denom(denom: impl AsRef<str>) -> Option<(TracePath, String)> {
    let prefixed_denom = PrefixedDenom::from_str(denom.as_ref()).ok()?;
    if prefixed_denom.trace_path.is_empty() {
        return None;
    }
    // The base token isn't decoded because it could be non Namada token
    Some((
        prefixed_denom.trace_path,
        prefixed_denom.base_denom.to_string(),
    ))
}

impl From<&IbcShieldedTransfer> for Memo {
    fn from(shielded: &IbcShieldedTransfer) -> Self {
        let bytes = shielded.serialize_to_vec();
        HEXUPPER.encode(&bytes).into()
    }
}

impl From<IbcShieldedTransfer> for Memo {
    fn from(shielded: IbcShieldedTransfer) -> Self {
        (&shielded).into()
    }
}

impl TryFrom<Memo> for IbcShieldedTransfer {
    type Error = Error;

    fn try_from(memo: Memo) -> Result<Self, Error> {
        let bytes = HEXUPPER
            .decode(memo.as_ref().as_bytes())
            .map_err(Error::DecodingHex)?;
        Self::try_from_slice(&bytes).map_err(Error::DecodingShieldedTransfer)
    }
}

/// Get the shielded transfer from the memo
pub fn get_shielded_transfer(
    event: &IbcEvent,
) -> Result<Option<IbcShieldedTransfer>, Error> {
    if event.event_type != EVENT_TYPE_PACKET {
        // This event is not for receiving a token
        return Ok(None);
    }
    let is_success =
        SuccessAttr::read_from_event_attributes(&event.attributes).is_ok();
    let is_shielded =
        event::ShieldedReceiver::read_from_event_attributes(&event.attributes)
            .is_ok();
    if !is_success || !is_shielded {
        return Ok(None);
    }

    event::ShieldedTransfer::read_opt_from_event_attributes(&event.attributes)
        .map_err(Error::Event)
}
