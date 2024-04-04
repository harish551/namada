use core::str::FromStr;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::path::PathBuf;
use std::sync::Arc;

use borsh::{BorshDeserialize, BorshSerialize};
use masp_primitives::sapling::keys::FullViewingKey;
use masp_primitives::sapling::{Diversifier, ViewingKey};
use masp_primitives::transaction::components::I128Sum;
use masp_primitives::transaction::Transaction;
use masp_primitives::zip32::{ExtendedFullViewingKey, ExtendedSpendingKey};
use masp_proofs::prover::LocalTxProver;
use namada_core::address::Address;
use namada_core::storage::{BlockHeight, Epoch, IndexedTx, Key, TxIndex};
use namada_core::token::Transfer;
use namada_ibc::IbcMessage;
use namada_tx::data::{TxResult, WrapperTx};
use namada_tx::Tx;
use rand_core::{CryptoRng, RngCore};
use tokio::sync::mpsc::{Receiver, Sender};

use crate::error::{Error, QueryError};
use crate::io::Io;
use crate::masp::shielded_ctx::ShieldedContext;
use crate::masp::types::{IndexedNoteEntry, PVKs, Unscanned};
use crate::masp::{ENV_VAR_MASP_PARAMS_DIR, VERIFIYING_KEYS};
use crate::queries::Client;
use crate::{MaybeSend, MaybeSync};

/// Make sure the MASP params are present and load verifying keys into memory
pub fn preload_verifying_keys() -> &'static PVKs {
    &VERIFIYING_KEYS
}

pub(super) fn load_pvks() -> &'static PVKs {
    &VERIFIYING_KEYS
}

/// Get the path to MASP parameters from [`ENV_VAR_MASP_PARAMS_DIR`] env var or
/// use the default.
pub fn get_params_dir() -> PathBuf {
    if let Ok(params_dir) = env::var(ENV_VAR_MASP_PARAMS_DIR) {
        println!("Using {} as masp parameter folder.", params_dir);
        PathBuf::from(params_dir)
    } else {
        masp_proofs::default_params_folder().unwrap()
    }
}

/// Abstracts platform specific details away from the logic of shielded pool
/// operations.
#[cfg_attr(feature = "async-send", async_trait::async_trait)]
#[cfg_attr(not(feature = "async-send"), async_trait::async_trait(?Send))]
pub trait ShieldedUtils:
    Sized + BorshDeserialize + BorshSerialize + Default + Clone
{
    /// Get a MASP transaction prover
    fn local_tx_prover(&self) -> LocalTxProver;

    /// Load up the currently saved ShieldedContext
    async fn load<U: ShieldedUtils + MaybeSend>(
        &self,
        ctx: &mut ShieldedContext<U>,
        force_confirmed: bool,
    ) -> std::io::Result<()>;

    /// Save the given ShieldedContext for future loads
    async fn save<U: ShieldedUtils + MaybeSync>(
        &self,
        ctx: &ShieldedContext<U>,
    ) -> std::io::Result<()>;
}

/// Make a ViewingKey that can view notes encrypted by given ExtendedSpendingKey
pub fn to_viewing_key(esk: &ExtendedSpendingKey) -> FullViewingKey {
    ExtendedFullViewingKey::from(esk).fvk
}

/// Generate a valid diversifier, i.e. one that has a diversified base. Return
/// also this diversified base.
pub fn find_valid_diversifier<R: RngCore + CryptoRng>(
    rng: &mut R,
) -> (Diversifier, masp_primitives::jubjub::SubgroupPoint) {
    let mut diversifier;
    let g_d;
    // Keep generating random diversifiers until one has a diversified base
    loop {
        let mut d = [0; 11];
        rng.fill_bytes(&mut d);
        diversifier = Diversifier(d);
        if let Some(val) = diversifier.g_d() {
            g_d = val;
            break;
        }
    }
    (diversifier, g_d)
}

/// Determine if using the current note would actually bring us closer to our
/// target
pub fn is_amount_required(src: I128Sum, dest: I128Sum, delta: I128Sum) -> bool {
    let gap = dest - src;
    for (asset_type, value) in gap.components() {
        if *value > 0 && delta[asset_type] > 0 {
            return true;
        }
    }
    false
}

/// An extension of Option's cloned method for pair types
pub(super) fn cloned_pair<T: Clone, U: Clone>((a, b): (&T, &U)) -> (T, U) {
    (a.clone(), b.clone())
}

/// Extract the payload from the given Tx object
pub(super) fn extract_payload(
    tx: Tx,
    wrapper: &mut Option<WrapperTx>,
    transfer: &mut Option<Transfer>,
) -> Result<(), Error> {
    *wrapper = tx.header.wrapper();
    let _ = tx.data().map(|signed| {
        Transfer::try_from_slice(&signed[..]).map(|tfer| *transfer = Some(tfer))
    });
    Ok(())
}

// Retrieves all the indexes and tx events at the specified height which refer
// to a valid masp transaction. If an index is given, it filters only the
// transactions with an index equal or greater to the provided one.
pub(super) async fn get_indexed_masp_events_at_height<C: Client>(
    client: &C,
    height: BlockHeight,
    first_idx_to_query: Option<TxIndex>,
) -> Result<Option<Vec<(TxIndex, crate::tendermint::abci::Event)>>, Error> {
    let first_idx_to_query = first_idx_to_query.unwrap_or_default();

    Ok(client
        .block_results(height.0 as u32)
        .await
        .map_err(|e| Error::from(QueryError::General(e.to_string())))?
        .end_block_events
        .map(|events| {
            events
                .into_iter()
                .filter_map(|event| {
                    let tx_index =
                        event.attributes.iter().find_map(|attribute| {
                            if attribute.key == "is_valid_masp_tx" {
                                Some(TxIndex(
                                    u32::from_str(&attribute.value).unwrap(),
                                ))
                            } else {
                                None
                            }
                        });

                    match tx_index {
                        Some(idx) => {
                            if idx >= first_idx_to_query {
                                Some((idx, event))
                            } else {
                                None
                            }
                        }
                        None => None,
                    }
                })
                .collect::<Vec<_>>()
        }))
}

pub(super) enum ExtractShieldedActionArg<'args, C: Client> {
    Event(&'args crate::tendermint::abci::Event),
    Request((&'args C, BlockHeight, Option<TxIndex>)),
}

/// Extract the relevant shield portions of a [`Tx`], if any.
pub(super) async fn extract_masp_tx<'args, C: Client + Sync>(
    tx: &Tx,
    action_arg: ExtractShieldedActionArg<'args, C>,
    check_header: bool,
) -> Result<(BTreeSet<namada_core::storage::Key>, Transaction), Error> {
    let maybe_transaction = if check_header {
        let tx_header = tx.header();
        // NOTE: simply looking for masp sections attached to the tx
        // is not safe. We don't validate the sections attached to a
        // transaction se we could end up with transactions carrying
        // an unnecessary masp section. We must instead look for the
        // required masp sections in the signed commitments (hashes)
        // of the transactions' headers/data sections
        if let Some(wrapper_header) = tx_header.wrapper() {
            let hash =
                wrapper_header.unshield_section_hash.ok_or_else(|| {
                    Error::Other(
                        "Missing expected fee unshielding section hash"
                            .to_string(),
                    )
                })?;

            let masp_transaction = tx
                .get_section(&hash)
                .ok_or_else(|| {
                    Error::Other("Missing expected masp section".to_string())
                })?
                .masp_tx()
                .ok_or_else(|| {
                    Error::Other("Missing masp transaction".to_string())
                })?;

            // We use the changed keys instead of the Transfer object
            // because those are what the masp validity predicate works on
            let changed_keys =
                if let ExtractShieldedActionArg::Event(tx_event) = action_arg {
                    let tx_result_str = tx_event
                        .attributes
                        .iter()
                        .find_map(|attr| {
                            if attr.key == "inner_tx" {
                                Some(&attr.value)
                            } else {
                                None
                            }
                        })
                        .ok_or_else(|| {
                            Error::Other(
                                "Missing required tx result in event"
                                    .to_string(),
                            )
                        })?;
                    TxResult::from_str(tx_result_str)
                        .map_err(|e| Error::Other(e.to_string()))?
                        .changed_keys
                } else {
                    BTreeSet::default()
                };

            Some((changed_keys, masp_transaction))
        } else {
            None
        }
    } else {
        None
    };

    let result = if let Some(tx) = maybe_transaction {
        tx
    } else {
        // Expect decrypted transaction
        let tx_data = tx
            .data()
            .ok_or_else(|| Error::Other("Missing data section".to_string()))?;
        match Transfer::try_from_slice(&tx_data) {
            Ok(transfer) => {
                let masp_transaction = tx
                    .get_section(&transfer.shielded.ok_or_else(|| {
                        Error::Other("Missing masp section hash".to_string())
                    })?)
                    .ok_or_else(|| {
                        Error::Other(
                            "Missing masp section in transaction".to_string(),
                        )
                    })?
                    .masp_tx()
                    .ok_or_else(|| {
                        Error::Other("Missing masp transaction".to_string())
                    })?;

                // We use the changed keys instead of the Transfer object
                // because those are what the masp validity predicate works
                // on
                let changed_keys =
                    if let ExtractShieldedActionArg::Event(tx_event) =
                        action_arg
                    {
                        let tx_result_str = tx_event
                            .attributes
                            .iter()
                            .find_map(|attr| {
                                if attr.key == "inner_tx" {
                                    Some(&attr.value)
                                } else {
                                    None
                                }
                            })
                            .ok_or_else(|| {
                                Error::Other(
                                    "Missing required tx result in event"
                                        .to_string(),
                                )
                            })?;
                        TxResult::from_str(tx_result_str)
                            .map_err(|e| Error::Other(e.to_string()))?
                            .changed_keys
                    } else {
                        BTreeSet::default()
                    };
                (changed_keys, masp_transaction)
            }
            Err(_) => {
                // This should be a MASP over IBC transaction, it
                // could be a ShieldedTransfer or an Envelope
                // message, need to try both

                extract_payload_from_shielded_action::<C>(&tx_data, action_arg)
                    .await?
            }
        }
    };
    Ok(result)
}

// Extract the changed keys and Transaction objects from a masp over ibc message
pub(super) async fn extract_payload_from_shielded_action<'args, C: Client>(
    tx_data: &[u8],
    mut args: ExtractShieldedActionArg<'args, C>,
) -> Result<(BTreeSet<namada_core::storage::Key>, Transaction), Error> {
    let message = namada_ibc::decode_message(tx_data)
        .map_err(|e| Error::Other(e.to_string()))?;

    let result = match message {
        IbcMessage::ShieldedTransfer(msg) => {
            let tx_event = match args {
                ExtractShieldedActionArg::Event(event) => event,
                ExtractShieldedActionArg::Request(_) => {
                    return Err(Error::Other(
                        "Unexpected event request for ShieldedTransfer"
                            .to_string(),
                    ));
                }
            };

            let changed_keys = tx_event
                .attributes
                .iter()
                .find_map(|attribute| {
                    if attribute.key == "inner_tx" {
                        let tx_result =
                            TxResult::from_str(&attribute.value).unwrap();
                        Some(tx_result.changed_keys)
                    } else {
                        None
                    }
                })
                .ok_or_else(|| {
                    Error::Other(
                        "Couldn't find changed keys in the event for the \
                         provided transaction"
                            .to_string(),
                    )
                })?;

            (changed_keys, msg.shielded_transfer.masp_tx)
        }
        IbcMessage::Envelope(_) => {
            let tx_event = match args {
                ExtractShieldedActionArg::Event(event) => {
                    std::borrow::Cow::Borrowed(event)
                }
                ExtractShieldedActionArg::Request((
                    ref mut client,
                    height,
                    index,
                )) => std::borrow::Cow::Owned(
                    get_indexed_masp_events_at_height::<C>(
                        client, height, index,
                    )
                    .await?
                    .ok_or_else(|| {
                        Error::Other(format!(
                            "Missing required ibc event at block height {}",
                            height
                        ))
                    })?
                    .first()
                    .ok_or_else(|| {
                        Error::Other(format!(
                            "Missing required ibc event at block height {}",
                            height
                        ))
                    })?
                    .1
                    .to_owned(),
                ),
            };

            tx_event
                .attributes
                .iter()
                .find_map(|attribute| {
                    if attribute.key == "inner_tx" {
                        let tx_result =
                            TxResult::from_str(&attribute.value).unwrap();
                        for ibc_event in &tx_result.ibc_events {
                            let event =
                                namada_core::ibc::get_shielded_transfer(
                                    ibc_event,
                                )
                                .ok()
                                .flatten();
                            if let Some(transfer) = event {
                                return Some((
                                    tx_result.changed_keys,
                                    transfer.masp_tx,
                                ));
                            }
                        }
                        None
                    } else {
                        None
                    }
                })
                .ok_or_else(|| {
                    Error::Other(
                        "Couldn't deserialize masp tx to ibc message envelope"
                            .to_string(),
                    )
                })?
        }
        _ => {
            return Err(Error::Other(
                "Couldn't deserialize masp tx to a valid ibc message"
                    .to_string(),
            ));
        }
    };

    Ok(result)
}

/// A channel-like struct for "sending" newly fetched blocks
/// to the scanning algorithm.
///
/// Holds a pointer to the unscanned cache which it can append to.
/// Furthermore, has an actual channel for keeping track if
/// 1. The process in possession of the channel is still alive
/// 2. Quickly updating the latest block height scanned.
#[derive(Clone)]
pub(super) struct FetchQueueSender {
    cache: Unscanned,
    last_fetched: flume::Sender<BlockHeight>,
}

/// A channel-like struct for "receiving" new fetched
/// blocks for the scanning algorithm.
///
/// This is implemented as an iterator for the scanning
/// algorithm. This receiver pulls from the cache until
/// it is empty. It then waits until new entries appear
/// in the cache or the sender hangs up.
#[derive(Clone)]
pub(super) struct FetchQueueReceiver {
    cache: Unscanned,
    last_fetched: flume::Receiver<BlockHeight>,
    last_query_height: BlockHeight,
}

impl FetchQueueReceiver {
    /// Check if the sender has hung up. If so, manually calculate the latest
    /// height fetched. Otherwise, update the latest height fetched with the
    /// data provided by the sender.
    fn sender_alive(&self) -> bool {
        self.last_fetched.sender_count() > 0
    }
}

impl Iterator for FetchQueueReceiver {
    type Item = IndexedNoteEntry;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(entry) = self.cache.pop_first() {
            Some(entry)
        } else {
            while self.sender_alive() {
                if let Some(entry) = self.cache.pop_first() {
                    return Some(entry);
                }
            }
            None
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let size = (self.last_query_height - self.cache.first_height().into()).0
            as usize;
        (size, Some(size))
    }
}

impl FetchQueueSender {
    pub(super) fn contains_height(&self, height: u64) -> bool {
        self.cache.contains_height(height)
    }

    pub(super) fn send(&mut self, data: IndexedNoteEntry) {
        self.last_fetched.send(data.0.height).unwrap();
        self.cache.insert(data);
    }
}

/// A convenience for creating a channel for fetching blocks.
pub mod fetch_channel {
    use namada_core::storage::BlockHeight;

    use super::{FetchQueueReceiver, FetchQueueSender, Unscanned};
    pub(in super::super) fn new(
        cache: Unscanned,
        last_query_height: BlockHeight,
    ) -> (FetchQueueSender, FetchQueueReceiver) {
        let (fetch_send, fetch_recv) = flume::unbounded();
        (
            FetchQueueSender {
                cache: cache.clone(),
                last_fetched: fetch_send,
            },
            FetchQueueReceiver {
                cache: cache.clone(),
                last_fetched: fetch_recv,
                last_query_height,
            },
        )
    }
}

enum Action<U: ShieldedUtils> {
    Complete,
    Data(Arc<futures_locks::Mutex<ShieldedContext<U>>>, BlockHeight),
}
pub struct TaskManager<U: ShieldedUtils> {
    action: Receiver<Action<U>>,
    pub(super) latest_height: BlockHeight,
}

#[derive(Clone)]
pub(super) struct TaskManagerChannel<U: ShieldedUtils> {
    action: Sender<Action<U>>,
    ctx: Arc<futures_locks::Mutex<ShieldedContext<U>>>,
}

impl<U: ShieldedUtils> TaskManager<U> {
    /// Create a client proxy and spawn a process to forward
    /// proxy requests.
    pub(super) fn new(
        ctx: ShieldedContext<U>,
    ) -> (TaskManagerChannel<U>, Self) {
        let (save_send, save_recv) = tokio::sync::mpsc::channel(100);
        (
            TaskManagerChannel {
                action: save_send,
                ctx: Arc::new(futures_locks::Mutex::new(ctx)),
            },
            TaskManager {
                action: save_recv,
                latest_height: Default::default(),
            },
        )
    }

    pub async fn run(&mut self) {
        while let Some(action) = self.action.recv().await {
            match action {
                Action::Complete => return,
                Action::Data(data, height) => {
                    self.latest_height = height;
                    let locked = data.lock().await;
                    _ = locked.save().await;
                }
            }
        }
    }
}

impl<U: ShieldedUtils> TaskManagerChannel<U> {

    pub(super) fn complete(&self) {
        self.action.blocking_send(Action::Complete).unwrap()
    }

    pub(super) fn save(&self, latest_height: BlockHeight) {
        self.action
            .blocking_send(Action::Data(self.ctx.clone(), latest_height))
            .unwrap();
    }

    pub(super) fn update_witness_map(
        &self,
        indexed_tx: IndexedTx,
        stx: &Transaction,
    ) -> Result<(), Error> {
        let mut locked = self.acquire();
        let res = locked.update_witness_map(indexed_tx, stx);
        if res.is_err() {
            self.complete()
        }
        res
    }

    pub(super) fn scan_tx(
        &self,
        indexed_tx: IndexedTx,
        epoch: Epoch,
        tx: &BTreeSet<Key>,
        stx: &Transaction,
        vk: &ViewingKey,
        native_token: Address,
    ) -> Result<(), Error> {
        let mut locked = self.acquire();
        let res = locked.scan_tx(indexed_tx, epoch, tx, stx, vk, native_token);
        if res.is_err() {
            self.complete();
        }
        res
    }

    pub(super) fn get_vk_heights(
        &self,
    ) -> BTreeMap<ViewingKey, Option<IndexedTx>> {
        let mut locked = self.acquire();
        let mut vk_heights = BTreeMap::new();
        std::mem::swap(&mut vk_heights, &mut locked.vk_heights);
        vk_heights
    }

    pub(super) fn set_vk_heights(
        &self,
        mut vk_heights: BTreeMap<ViewingKey, Option<IndexedTx>>,
    ) {
        let mut locked = self.acquire();
        std::mem::swap(&mut vk_heights, &mut locked.vk_heights);
    }

    /// Kids, don't try this at home.
    fn acquire(&self) -> futures_locks::MutexGuard<ShieldedContext<U>> {
        loop {
            if let Ok(ctx) = self.ctx.try_lock() {
                return ctx;
            }
            std::hint::spin_loop();
        }
    }
}

/// An enum to indicate how to log sync progress depending on
/// whether sync is currently fetch or scanning blocks.
#[derive(Debug, Copy, Clone)]
pub enum ProgressType {
    Fetch,
    Scan,
}

pub trait ProgressLogger<IO: Io> {
    fn io(&self) -> &IO;

    fn fetch<I>(&self, items: I) -> impl Iterator<Item = u64>
    where
        I: Iterator<Item = u64>;

    fn scan<I>(&self, items: I) -> impl Iterator<Item = IndexedNoteEntry>
    where
        I: Iterator<Item = IndexedNoteEntry>;

    fn left_to_fetch(&self) -> usize;
}