//! Gas related events.

use namada_core::event::extend::EventAttributeEntry;

use super::Gas;

/// Extend an [`Event`] with gas used data.
pub struct GasUsed(pub Gas);

impl EventAttributeEntry<'static> for GasUsed {
    type Value = Gas;
    type ValueOwned = Self::Value;

    const KEY: &'static str = "gas_used";

    fn into_value(self) -> Self::Value {
        self.0
    }
}
