//! Ledger events

pub mod extend;

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::{self, Display};
use std::ops::{Deref, Index, IndexMut};
use std::str::FromStr;

use namada_macros::BorshDeserializer;
#[cfg(feature = "migrations")]
use namada_migrations::*;
use thiserror::Error;

use crate::borsh::{BorshDeserialize, BorshSerialize};
use crate::ethereum_structs::{BpTransferStatus, EthBridgeEvent};
use crate::ibc::IbcEvent;

/// An event to be emitted in Namada.
pub trait EventToEmit: Into<Event> {
    /// The domain of the event to emit.
    ///
    /// This may be used to group events of a certain kind.
    const DOMAIN: &'static str;

    /// Utility method to return the value of [`Self::DOMAIN`].
    #[inline(always)]
    fn domain(&self) -> &'static str {
        Self::DOMAIN
    }
}

impl EventToEmit for Event {
    const DOMAIN: &'static str = "generic";
}

impl EventToEmit for IbcEvent {
    const DOMAIN: &'static str = "ibc";
}

impl EventToEmit for EthBridgeEvent {
    const DOMAIN: &'static str = "eth-bridge";
}

/// Used in sub-systems that may emit events.
pub trait EmitEvents {
    /// Emit a single [event](Event).
    fn emit<E>(&mut self, event: E)
    where
        E: EventToEmit;

    /// Emit a batch of [events](Event).
    fn emit_many<B, E>(&mut self, event_batch: B)
    where
        B: IntoIterator<Item = E>,
        E: EventToEmit;
}

impl EmitEvents for Vec<Event> {
    #[inline]
    fn emit<E>(&mut self, event: E)
    where
        E: Into<Event>,
    {
        self.push(event.into());
    }

    /// Emit a batch of [events](Event).
    fn emit_many<B, E>(&mut self, event_batch: B)
    where
        B: IntoIterator<Item = E>,
        E: Into<Event>,
    {
        self.extend(event_batch.into_iter().map(Into::into));
    }
}

/// Indicates if an event is emitted do to
/// an individual Tx or the nature of a finalized block
#[derive(
    Clone,
    Debug,
    Eq,
    PartialEq,
    BorshSerialize,
    BorshDeserialize,
    BorshDeserializer,
)]
pub enum EventLevel {
    /// Indicates an event is to do with a finalized block.
    Block,
    /// Indicates an event is to do with an individual transaction.
    Tx,
}

impl Display for EventLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                EventLevel::Block => "block",
                EventLevel::Tx => "tx",
            }
        )
    }
}

/// Logical segmentation of an ABCI event kind.
#[derive(
    Clone,
    Debug,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    BorshDeserializer,
)]
#[repr(transparent)]
pub struct EventSegment {
    inner: Cow<'static, str>,
}

impl EventSegment {
    /// Instantiate a new [`EventSegment`].
    #[inline]
    pub fn new<D>(domain: D) -> Self
    where
        D: Into<Cow<'static, str>>,
    {
        Self {
            inner: domain.into(),
        }
    }

    /// Instantiate a new [`EventSegment`] from a static string.
    pub const fn new_static(domain: &'static str) -> Self {
        Self {
            inner: Cow::Borrowed(domain),
        }
    }
}

impl Deref for EventSegment {
    type Target = str;

    #[inline(always)]
    fn deref(&self) -> &str {
        &self.inner
    }
}

/// The domain of an [`Event`]. This represents the most general
/// category an event can fit into (e.g. IBC, Ethereum Bridge).
#[derive(
    Clone,
    Debug,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    BorshDeserializer,
)]
#[repr(transparent)]
pub struct EventDomain {
    inner: EventSegment,
}

impl From<EventSegment> for EventDomain {
    #[inline(always)]
    fn from(inner: EventSegment) -> Self {
        Self { inner }
    }
}

impl Deref for EventDomain {
    type Target = str;

    #[inline(always)]
    fn deref(&self) -> &str {
        &self.inner
    }
}

/// Custom events that can be queried from Tendermint
/// using a websocket client
#[derive(
    Clone,
    Debug,
    Eq,
    PartialEq,
    BorshSerialize,
    BorshDeserialize,
    BorshDeserializer,
)]
pub struct Event {
    /// The type of event.
    pub event_type: EventType,
    /// The level of the event - whether it relates to a block or an individual
    /// transaction.
    pub level: EventLevel,
    /// Key-value attributes of the event.
    pub attributes: HashMap<String, String>,
}

impl Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: print attributes, too
        write!(f, "{} in {}", self.event_type, self.level)
    }
}

/// The two types of custom events we currently use
#[derive(
    Clone,
    Debug,
    Eq,
    PartialEq,
    BorshSerialize,
    BorshDeserialize,
    BorshDeserializer,
)]
pub enum EventType {
    /// The transaction was accepted to be included in a block
    Accepted,
    /// The transaction was applied during block finalization
    Applied,
    /// The IBC transaction was applied during block finalization
    // TODO: create type-safe wrapper for all ibc event kinds
    Ibc(String),
    /// The proposal that has been executed
    Proposal,
    /// The pgf payment
    PgfPayment,
    /// Ethereum Bridge event
    EthereumBridge,
}

impl Display for EventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EventType::Accepted => write!(f, "accepted"),
            EventType::Applied => write!(f, "applied"),
            EventType::Ibc(t) => write!(f, "{}", t),
            EventType::Proposal => write!(f, "proposal"),
            EventType::PgfPayment => write!(f, "pgf_payment"),
            EventType::EthereumBridge => write!(f, "ethereum_bridge"),
        }?;
        Ok(())
    }
}

impl FromStr for EventType {
    type Err = EventError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "accepted" => Ok(EventType::Accepted),
            "applied" => Ok(EventType::Applied),
            "proposal" => Ok(EventType::Proposal),
            "pgf_payments" => Ok(EventType::PgfPayment),
            // <IBC>
            "update_client" => Ok(EventType::Ibc("update_client".to_string())),
            "send_packet" => Ok(EventType::Ibc("send_packet".to_string())),
            "write_acknowledgement" => {
                Ok(EventType::Ibc("write_acknowledgement".to_string()))
            }
            // </IBC>
            "ethereum_bridge" => Ok(EventType::EthereumBridge),
            _ => Err(EventError::InvalidEventType),
        }
    }
}

/// Errors to do with emitting events.
#[derive(Error, Debug, Clone)]
pub enum EventError {
    /// Error when parsing an event type
    #[error("Invalid event type")]
    InvalidEventType,
    /// Error when parsing attributes from an event JSON.
    #[error("Json missing `attributes` field")]
    MissingAttributes,
    /// Missing key in attributes.
    #[error("Attributes missing key: {0}")]
    MissingKey(String),
    /// Missing value in attributes.
    #[error("Attributes missing value: {0}")]
    MissingValue(String),
}

impl Event {
    /// Create an accepted tx event with empty attributes.
    pub fn accepted_tx() -> Self {
        Self {
            event_type: EventType::Accepted,
            level: EventLevel::Tx,
            attributes: HashMap::new(),
        }
    }

    /// Create an applied tx event with empty attributes.
    pub fn applied_tx() -> Self {
        Self {
            event_type: EventType::Applied,
            level: EventLevel::Tx,
            attributes: HashMap::new(),
        }
    }

    /// Check if the events keys contains a given string
    pub fn contains_key(&self, key: &str) -> bool {
        self.attributes.contains_key(key)
    }

    /// Get the value corresponding to a given key, if it exists.
    /// Else return None.
    pub fn get(&self, key: &str) -> Option<&String> {
        self.attributes.get(key)
    }

    /// Extend this [`Event`] with additional data.
    #[inline]
    pub fn extend<DATA>(&mut self, data: DATA) -> &mut Self
    where
        DATA: extend::ExtendEvent,
    {
        data.extend_event(self);
        self
    }
}

impl From<EthBridgeEvent> for Event {
    #[inline]
    fn from(event: EthBridgeEvent) -> Event {
        Self::from(&event)
    }
}

impl From<&EthBridgeEvent> for Event {
    fn from(event: &EthBridgeEvent) -> Event {
        match event {
            EthBridgeEvent::BridgePool { tx_hash, status } => Event {
                event_type: EventType::EthereumBridge,
                level: EventLevel::Tx,
                attributes: {
                    let mut attrs = HashMap::new();
                    attrs.insert(
                        "kind".into(),
                        match status {
                            BpTransferStatus::Relayed => "bridge_pool_relayed",
                            BpTransferStatus::Expired => "bridge_pool_expired",
                        }
                        .into(),
                    );
                    attrs.insert("tx_hash".into(), tx_hash.to_string());
                    attrs
                },
            },
        }
    }
}

impl Index<&str> for Event {
    type Output = String;

    fn index(&self, index: &str) -> &Self::Output {
        &self.attributes[index]
    }
}

impl IndexMut<&str> for Event {
    fn index_mut(&mut self, index: &str) -> &mut Self::Output {
        let entry = self.attributes.entry(index.into()).or_default();
        &mut *entry
    }
}

impl From<IbcEvent> for Event {
    fn from(ibc_event: IbcEvent) -> Self {
        Self {
            event_type: EventType::Ibc(ibc_event.event_type),
            level: EventLevel::Tx,
            attributes: ibc_event.attributes,
        }
    }
}

/// Convert our custom event into the necessary tendermint proto type
impl From<Event> for crate::tendermint_proto::v0_37::abci::Event {
    fn from(event: Event) -> Self {
        Self {
            r#type: event.event_type.to_string(),
            attributes: event
                .attributes
                .into_iter()
                .map(|(key, value)| {
                    crate::tendermint_proto::v0_37::abci::EventAttribute {
                        key,
                        value,
                        index: true,
                    }
                })
                .collect(),
        }
    }
}
