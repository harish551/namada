//! IBC event related types

use std::cmp::Ordering;
use std::collections::HashMap;

use namada_macros::BorshDeserializer;
#[cfg(feature = "migrations")]
use namada_migrations::*;
use serde::{Deserialize, Serialize};

use crate::borsh::*;
use crate::event::extend::EventAttributeEntry;
use crate::event::{Event, EventError, EventToEmit as _};
use crate::ibc::core::client::types::events::{
    CLIENT_ID_ATTRIBUTE_KEY, CONSENSUS_HEIGHTS_ATTRIBUTE_KEY,
};
use crate::ibc::core::client::types::Height as IbcHeight;
use crate::ibc::core::handler::types::events::IbcEvent as RawIbcEvent;
use crate::ibc::core::host::types::identifiers::{
    ChannelId, ClientId as IbcClientId, ConnectionId as IbcConnectionId,
    PortId, Sequence,
};
use crate::tendermint::abci::Event as AbciEvent;

pub mod types {
    //! IBC event types.

    use std::borrow::Cow;

    use super::IbcEvent;
    use crate::event::{new_event_type_of, EventSegment, EventType};
    use crate::ibc::core::client::types::events::UPDATE_CLIENT_EVENT;

    /// Update client.
    pub const UPDATE_CLIENT: EventType =
        new_event_type_of::<IbcEvent>(Cow::Borrowed({
            const SEGMENT: &[EventSegment] =
                &[EventSegment::new_static(UPDATE_CLIENT_EVENT)];
            SEGMENT
        }));
}

/// Wrapped IbcEvent
#[derive(
    Debug,
    Clone,
    BorshSerialize,
    BorshDeserialize,
    BorshDeserializer,
    BorshSchema,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
)]
pub struct IbcEvent {
    /// The IBC event type
    pub event_type: String,
    /// The attributes of the IBC event
    pub attributes: HashMap<String, String>,
}

impl TryFrom<Event> for IbcEvent {
    type Error = EventError;

    fn try_from(namada_event: Event) -> std::result::Result<Self, Self::Error> {
        if namada_event.event_type.domain != IbcEvent::DOMAIN {
            return Err(EventError::InvalidEventType);
        }

        let event_type = namada_event.event_type.sub_domain();

        if !matches!(
            event_type.as_str(),
            // TODO: add other ibc event types that we use in namada
            "update_client" | "send_packet" | "write_acknowledgement"
        ) {
            return Err(EventError::InvalidEventType);
        }

        Ok(Self {
            event_type,
            attributes: namada_event.attributes,
        })
    }
}

impl std::cmp::PartialOrd for IbcEvent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl std::cmp::Ord for IbcEvent {
    fn cmp(&self, other: &Self) -> Ordering {
        // should not compare the same event type
        self.event_type.cmp(&other.event_type)
    }
}

impl std::fmt::Display for IbcEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let attributes = self
            .attributes
            .iter()
            .map(|(k, v)| format!("{}: {};", k, v))
            .collect::<Vec<String>>()
            .join(", ");
        write!(
            f,
            "Event type: {}, Attributes: {}",
            self.event_type, attributes
        )
    }
}

impl TryFrom<RawIbcEvent> for IbcEvent {
    type Error = super::Error;

    fn try_from(e: RawIbcEvent) -> Result<Self, super::Error> {
        let event_type = e.event_type().to_string();
        let abci_event =
            AbciEvent::try_from(e).map_err(super::Error::IbcEvent)?;
        let attributes: HashMap<_, _> = abci_event
            .attributes
            .iter()
            .map(|tag| (tag.key.to_string(), tag.value.to_string()))
            .collect();
        Ok(Self {
            event_type,
            attributes,
        })
    }
}

/// Extend an [`Event`] with packet sequence data.
pub struct PacketSequence(pub Sequence);

impl EventAttributeEntry<'static> for PacketSequence {
    type Value = Sequence;
    type ValueOwned = Self::Value;

    const KEY: &'static str = "packet_sequence";

    fn into_value(self) -> Self::Value {
        self.0
    }
}

/// Extend an [`Event`] with packet source port data.
pub struct PacketSrcPort(pub PortId);

impl EventAttributeEntry<'static> for PacketSrcPort {
    type Value = PortId;
    type ValueOwned = Self::Value;

    const KEY: &'static str = "packet_src_port";

    fn into_value(self) -> Self::Value {
        self.0
    }
}

/// Extend an [`Event`] with packet source channel data.
pub struct PacketSrcChannel(pub ChannelId);

impl EventAttributeEntry<'static> for PacketSrcChannel {
    type Value = ChannelId;
    type ValueOwned = Self::Value;

    const KEY: &'static str = "packet_src_channel";

    fn into_value(self) -> Self::Value {
        self.0
    }
}

/// Extend an [`Event`] with packet destination port data.
pub struct PacketDstPort(pub PortId);

impl EventAttributeEntry<'static> for PacketDstPort {
    type Value = PortId;
    type ValueOwned = Self::Value;

    const KEY: &'static str = "packet_dst_port";

    fn into_value(self) -> Self::Value {
        self.0
    }
}

/// Extend an [`Event`] with packet destination channel data.
pub struct PacketDstChannel(pub ChannelId);

impl EventAttributeEntry<'static> for PacketDstChannel {
    type Value = ChannelId;
    type ValueOwned = Self::Value;

    const KEY: &'static str = "packet_dst_channel";

    fn into_value(self) -> Self::Value {
        self.0
    }
}

/// Extend an [`Event`] with client id data.
pub struct ClientId(pub IbcClientId);

impl EventAttributeEntry<'static> for ClientId {
    type Value = IbcClientId;
    type ValueOwned = Self::Value;

    const KEY: &'static str = CLIENT_ID_ATTRIBUTE_KEY;

    fn into_value(self) -> Self::Value {
        self.0
    }
}

/// Extend an [`Event`] with consensus heights data.
pub struct ConsensusHeights(pub IbcHeight);

impl EventAttributeEntry<'static> for ConsensusHeights {
    type Value = IbcHeight;
    type ValueOwned = Self::Value;

    const KEY: &'static str = CONSENSUS_HEIGHTS_ATTRIBUTE_KEY;

    fn into_value(self) -> Self::Value {
        self.0
    }
}

/// Extend an [`Event`] with connection id data.
pub struct ConnectionId(pub IbcConnectionId);

impl EventAttributeEntry<'static> for ConnectionId {
    type Value = IbcConnectionId;
    type ValueOwned = Self::Value;

    const KEY: &'static str = "connection_id";

    fn into_value(self) -> Self::Value {
        self.0
    }
}
