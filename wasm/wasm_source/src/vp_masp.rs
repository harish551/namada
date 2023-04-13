use std::cmp::Ordering;

use masp_primitives::asset_type::AssetType;
use masp_primitives::transaction::components::Amount;
/// Multi-asset shielded pool VP.
use namada_vp_prelude::address::masp;
use namada_vp_prelude::storage::Epoch;
use namada_vp_prelude::*;

/// Convert Namada amount and token type to MASP equivalents
fn convert_amount(
    epoch: Epoch,
    token: &Address,
    val: token::Amount,
) -> (AssetType, Amount) {
    // Timestamp the chosen token with the current epoch
    let token_bytes = (token, epoch.0)
        .try_to_vec()
        .expect("token should serialize");
    // Generate the unique asset identifier from the unique token address
    let asset_type = AssetType::new(token_bytes.as_ref())
        .expect("unable to create asset type");
    // Combine the value and unit into one amount
    let amount = Amount::from_nonnegative(asset_type, u64::from(val))
        .expect("invalid value or asset type for amount");
    (asset_type, amount)
}

#[validity_predicate]
fn validate_tx(
    ctx: &Ctx,
    tx_data: Tx,
    addr: Address,
    keys_changed: BTreeSet<storage::Key>,
    verifiers: BTreeSet<Address>,
) -> VpResult {
    debug_log!(
        "vp_masp called with {} bytes data, address {}, keys_changed {:?}, \
         verifiers {:?}",
        tx_data.data().as_ref().map(|x| x.len()).unwrap_or(0),
        addr,
        keys_changed,
        verifiers,
    );

    let signed = tx_data;
    // Also get the data as bytes for the VM.
    let data = signed.data().as_ref().unwrap().clone();
    let transfer =
        token::Transfer::try_from_slice(&signed.data().unwrap()[..]).unwrap();

    let shielded = transfer.shielded.as_ref().map(|hash| {
        signed
            .get_section(&hash)
            .and_then(Section::masp_tx)
            .ok_or_err_msg("unable to find shielded section")
    }).transpose()?;
    if let Some(shielded_tx) = shielded {
        let mut transparent_tx_pool = Amount::zero();
        // The Sapling value balance adds to the transparent tx pool
        transparent_tx_pool += shielded_tx.sapling_value_balance();

        // Note that the asset type is timestamped so shields
        // where the shielded value has an incorrect timestamp
        // are automatically rejected
        let (_transp_asset, transp_amt) = convert_amount(
            ctx.get_block_epoch().unwrap(),
            &transfer.token,
            transfer.amount,
        );
        // Handle shielding/transparent input
        if transfer.source != masp() {
            // Non-masp sources add to transparent tx pool
            transparent_tx_pool += transp_amt.clone();
        }

        // Handle unshielding/transparent output
        if transfer.target != masp() {
            // Non-masp destinations subtract from transparent tx pool
            transparent_tx_pool -= transp_amt;
        }

        match transparent_tx_pool.partial_cmp(&Amount::zero()) {
            None | Some(Ordering::Less) => {
                debug_log!(
                    "Transparent transaction value pool must be nonnegative. \
                     Violation may be caused by transaction being constructed \
                     in previous epoch. Maybe try again."
                );
                // Section 3.4: The remaining value in the transparent
                // transaction value pool MUST be nonnegative.
                return reject();
            }
            _ => {}
        }
        // Do the expensive proof verification in the VM at the end.
        ctx.verify_masp(shielded_tx.try_to_vec().unwrap())
    } else {
        reject()
    }
}
