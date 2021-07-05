// Copyright 2019. The Tari Project
//
// Redistribution and use in source and binary forms, with or without modification, are permitted provided that the
// following conditions are met:
//
// 1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following
// disclaimer.
//
// 2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the
// following disclaimer in the documentation and/or other materials provided with the distribution.
//
// 3. Neither the name of the copyright holder nor the names of its contributors may be used to endorse or promote
// products derived from this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES,
// INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
// DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
// SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
// SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY,
// WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE
// USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

use crate::transactions::{
    fee::Fee,
    tari_amount::*,
    transaction::{
        TransactionInput,
        TransactionOutput,
        UnblindedOutput,
        MAX_TRANSACTION_INPUTS,
        MAX_TRANSACTION_OUTPUTS,
        MINIMUM_TRANSACTION_FEE,
    },
    transaction_protocol::{
        recipient::RecipientInfo,
        sender::{calculate_tx_id, RawTransactionInfo, SenderState, SenderTransactionProtocol},
        RewindData,
        TransactionMetadata,
    },
    types::{BlindingFactor, CryptoFactories, PrivateKey, PublicKey},
};
use digest::Digest;
use log::*;
use rand::rngs::OsRng;
use std::{
    cmp::max,
    collections::HashMap,
    fmt::{Debug, Error, Formatter},
};
use tari_crypto::{
    keys::{PublicKey as PublicKeyTrait, SecretKey},
    script::{ExecutionStack, TariScript},
    tari_utilities::fixed_set::FixedSet,
};

pub const LOG_TARGET: &str = "c::tx::tx_protocol::tx_initializer";

/// The SenderTransactionInitializer is a Builder that helps set up the initial state for the Sender party of a new
/// transaction Typically you don't instantiate this object directly. Rather use
/// ```ignore
/// # use crate::SenderTransactionProtocol;
/// SenderTransactionProtocol::new(1);
/// ```
/// which returns an instance of this builder. Once all the sender's information has been added via the builder
/// methods, you can call `build()` which will return a
#[derive(Debug)]
pub struct SenderTransactionProtocolBuilder {
    num_recipients: usize,
    amounts: FixedSet<MicroTari>,
    lock_height: Option<u64>,
    fee_per_gram: Option<MicroTari>,
    inputs: Vec<TransactionInput>,
    unblinded_inputs: Vec<UnblindedOutput>,
    outputs: Vec<UnblindedOutput>,
    script_offset_private_keys: Vec<PrivateKey>,
    change_secret: Option<BlindingFactor>,
    change_script: Option<TariScript>,
    change_input_data: Option<ExecutionStack>,
    change_script_private_key: Option<PrivateKey>,
    change_script_offset_private_key: Option<PrivateKey>,
    rewind_data: Option<RewindData>,
    offset: Option<BlindingFactor>,
    excess_blinding_factor: BlindingFactor,
    private_nonce: Option<PrivateKey>,
    message: Option<String>,
    prevent_fee_gt_amount: bool,
    recipient_scripts: FixedSet<TariScript>,
    recipient_script_offset_private_keys: FixedSet<PrivateKey>,
    unique_id: Option<Vec<u8>>
}

pub struct BuildError {
    pub builder: SenderTransactionProtocolBuilder,
    pub message: String,
}

impl Debug for BuildError {
    fn fmt(&self, f: &mut Formatter) -> Result<(), Error> {
        f.write_str(&self.message)
    }
}

impl SenderTransactionProtocolBuilder {
    pub fn new(num_recipients: usize) -> Self {
        Self {
            num_recipients,
            amounts: FixedSet::new(num_recipients),
            lock_height: None,
            fee_per_gram: None,
            inputs: Vec::new(),
            unblinded_inputs: Vec::new(),
            outputs: Vec::new(),
            script_offset_private_keys: vec![],
            change_secret: None,
            change_script: None,
            change_input_data: None,
            change_script_private_key: None,
            change_script_offset_private_key: None,
            rewind_data: None,
            offset: None,
            private_nonce: None,
            excess_blinding_factor: BlindingFactor::default(),
            message: None,
            prevent_fee_gt_amount: true,
            recipient_scripts: FixedSet::new(num_recipients),
            recipient_script_offset_private_keys: FixedSet::new(num_recipients),
            unique_id: None
        }
    }

    /// Set the fee per weight for the transaction. See (Fee::calculate)[Struct.Fee.html#calculate] for how the
    /// absolute fee is calculated from the fee-per-gram value.
    pub fn with_fee_per_gram(&mut self, fee_per_gram: MicroTari) -> &mut Self {
        self.fee_per_gram = Some(fee_per_gram);
        self
    }

    /// Set the amount to pay to the ith recipient. This method will silently fail if `receiver_index` >= num_receivers.
    pub fn with_amount(&mut self, receiver_index: usize, amount: MicroTari) -> &mut Self {
        self.amounts.set_item(receiver_index, amount);
        self
    }

    /// Set the spending script of the ith recipient's output, a script offset will be generated for this recipient at
    /// the same time. This method will silently fail if `receiver_index` >= num_receivers.
    pub fn with_recipient_script(
        &mut self,
        receiver_index: usize,
        script: TariScript,
        recipient_script_offset_private_key: PrivateKey,
    ) -> &mut Self {
        self.recipient_scripts.set_item(receiver_index, script);
        self.recipient_script_offset_private_keys
            .set_item(receiver_index, recipient_script_offset_private_key);
        self
    }

    /// Sets the minimum block height that this transaction will be mined.
    pub fn with_lock_height(&mut self, lock_height: u64) -> &mut Self {
        self.lock_height = Some(lock_height);
        self
    }

    /// Manually sets the offset value. If this is not called, a random offset will be used when `build()` is called.
    pub fn with_offset(&mut self, offset: BlindingFactor) -> &mut Self {
        self.offset = Some(offset);
        self
    }

    /// Adds an input to the transaction. The sender must provide the blinding factor that was used when the input
    /// was first set as an output. We don't check that the input and commitments match at this point.
    pub fn with_input(&mut self, utxo: TransactionInput, input: UnblindedOutput) -> &mut Self {
        self.inputs.push(utxo);
        self.excess_blinding_factor = &self.excess_blinding_factor - &input.spending_key;
        self.unblinded_inputs.push(input);
        self
    }

    /// As the Sender adds an output to the transaction. Because we are adding this output as the sender a
    /// script_offset_private_key needs to be provided with the output. This can be called multiple times
    pub fn with_output(&mut self, output: UnblindedOutput, script_offset_private_key: PrivateKey) -> &mut Self {
        self.excess_blinding_factor = &self.excess_blinding_factor + &output.spending_key;
        self.outputs.push(output);
        self.script_offset_private_keys.push(script_offset_private_key);
        self
    }

    /// Provide a blinding factor for the change output. The amount of change will automatically be calculated when
    /// the transaction is built.
    pub fn with_change_secret(&mut self, blinding_factor: BlindingFactor) -> &mut Self {
        self.change_secret = Some(blinding_factor);
        self
    }

    /// Provide the script data that will be used to spend the change output
    pub fn with_change_script(
        &mut self,
        script: TariScript,
        input_data: ExecutionStack,
        script_private_key: PrivateKey,
    ) -> &mut Self {
        self.change_script = Some(script);
        self.change_input_data = Some(input_data);
        self.change_script_private_key = Some(script_private_key);
        self
    }

    /// Provide the rewind data required for outputs (change and manually added sender outputs) to be rewindable.
    pub fn with_rewindable_outputs(&mut self, rewind_data: RewindData) -> &mut Self {
        self.rewind_data = Some(rewind_data);
        self
    }

    /// Provide the private nonce that will be used for the sender's partial signature for the transaction.
    pub fn with_private_nonce(&mut self, nonce: PrivateKey) -> &mut Self {
        self.private_nonce = Some(nonce);
        self
    }

    /// Provide a text message for receiver
    pub fn with_message(&mut self, message: String) -> &mut Self {
        self.message = Some(message);
        self
    }

    /// Enable or disable spending of an amount less than the fee
    pub fn with_prevent_fee_gt_amount(&mut self, prevent_fee_gt_amount: bool) -> &mut Self {
        self.prevent_fee_gt_amount = prevent_fee_gt_amount;
        self
    }

    /// Tries to make a change output with the given transaction parameters and add it to the set of outputs. The total
    /// fee, including the additional change output (if any) is returned along with the amount of change.
    /// The change output **always has default output features**.
    fn add_change_if_required(&mut self) -> Result<(MicroTari, MicroTari, Option<UnblindedOutput>), String> {
        // The number of outputs excluding a possible residual change output
        let num_outputs = self.outputs.len() + self.num_recipients;
        let num_inputs = self.inputs.len();
        let total_being_spent = self.unblinded_inputs.iter().map(|i| i.value).sum::<MicroTari>();
        let total_to_self = self.outputs.iter().map(|o| o.value).sum::<MicroTari>();
        let total_amount = self.amounts.sum().ok_or("Not all amounts have been provided")?;
        let fee_per_gram = self.fee_per_gram.ok_or("Fee per gram was not provided")?;
        let fee_without_change = Fee::calculate(fee_per_gram, 1, num_inputs, num_outputs);
        let fee_with_change = Fee::calculate(fee_per_gram, 1, num_inputs, num_outputs + 1);
        let extra_fee = fee_with_change - fee_without_change;
        // Subtract with a check on going negative
        let change_amount = total_being_spent.checked_sub(total_to_self + total_amount + fee_without_change);
        match change_amount {
            None => Err("You are spending more than you're providing".into()),
            Some(MicroTari(0)) => Ok((fee_without_change, MicroTari(0), None)),
            Some(v) => {
                let change_amount = v.checked_sub(extra_fee);
                let change_script_offset_private_key = PrivateKey::random(&mut OsRng);
                self.change_script_offset_private_key = Some(change_script_offset_private_key.clone());
//TODO: Add unique id if needed
                match change_amount {
                    // You can't win. Just add the change to the fee (which is less than the cost of adding another
                    // output and go without a change output
                    None => Ok((fee_without_change + v, MicroTari(0), None)),
                    Some(MicroTari(0)) => Ok((fee_without_change + v, MicroTari(0), None)),
                    Some(v) => {
                        let change_key = self
                            .change_secret
                            .as_ref()
                            .ok_or("Change spending key was not provided")?;
                        let change_unblinded_output = UnblindedOutput::new(
                            v,
                            change_key.clone(),
                            None,
                            self.change_script
                                .as_ref()
                                .ok_or("Change script was not provided")?
                                .clone(),
                            self.change_input_data
                                .as_ref()
                                .ok_or("Change script was not provided")?
                                .clone(),
                            0,
                            self.change_script_private_key
                                .as_ref()
                                .ok_or("Change script private key was not provided")?
                                .clone(),
                            PublicKey::from_secret_key(&change_script_offset_private_key),
                            None,
                            None
                        );
                        Ok((fee_with_change, v, Some(change_unblinded_output)))
                    },
                }
            },
        }
    }

    fn check_value<T>(name: &str, val: &Option<T>, vec: &mut Vec<String>) {
        if val.is_none() {
            vec.push(name.to_string());
        }
    }

    fn build_err<T>(self, msg: &str) -> Result<T, BuildError> {
        Err(BuildError {
            builder: self,
            message: msg.to_string(),
        })
    }

    fn calculate_amount_to_others(&self) -> MicroTari {
        self.amounts.clone().into_vec().iter().sum()
    }

    pub fn with_unique_id(mut self, unique_id: Vec<u8>) -> Self {
        self.unique_id = Some(unique_id);
        self
    }

    /// Construct a `SenderTransactionProtocol` instance in and appropriate state. The data stored
    /// in the struct is _moved_ into the new struct. If any data is missing, the `self` instance is returned in the
    /// error (so that you can continue building) along with a string listing the missing fields.
    /// If all the input data is present, but one or more fields are invalid, the function will return a
    /// `SenderTransactionProtocol` instance in the Failed state.
    pub fn build<D: Digest>(mut self, factories: &CryptoFactories) -> Result<SenderTransactionProtocol, BuildError> {
        // Compile a list of all data that is missing
        let mut message = Vec::new();
        Self::check_value("Missing Lock Height", &self.lock_height, &mut message);
        Self::check_value("Missing Fee per gram", &self.fee_per_gram, &mut message);
        Self::check_value("Missing Offset", &self.offset, &mut message);
        Self::check_value("Change script", &self.private_nonce, &mut message);
        Self::check_value("Change input data", &self.private_nonce, &mut message);
        Self::check_value("Change script private key", &self.private_nonce, &mut message);

        if !message.is_empty() {
            return self.build_err(&message.join(","));
        }
        if !self.amounts.is_full() {
            let size = self.amounts.size();
            return self.build_err(&*format!("Missing all {} amounts", size));
        }
        if !self.recipient_script_offset_private_keys.is_full() {
            let size = self.recipient_script_offset_private_keys.size();
            return self.build_err(&*format!("Missing {} recipient script offset private key/s", size));
        }
        if !self.recipient_scripts.is_full() {
            let size = self.recipient_scripts.size();
            return self.build_err(&*format!("Missing all {} recipient scripts", size));
        }
        if self.inputs.is_empty() {
            return self.build_err("A transaction cannot have zero inputs");
        }
        // Prevent overflow attacks by imposing sane limits on inputs
        if self.inputs.len() > MAX_TRANSACTION_INPUTS {
            return self.build_err("Too many inputs in transaction");
        }
        // Calculate the fee based on whether we need to add a residual change output or not
        let (total_fee, change, change_output) = match self.add_change_if_required() {
            Ok((fee, change, output)) => (fee, change, output),
            Err(e) => return self.build_err(&e),
        };
        debug!(
            target: LOG_TARGET,
            "Build transaction with Fee: {}. Change: {}. Output: {:?}", total_fee, change, change_output,
        );
        // Some checks on the fee
        if total_fee < MINIMUM_TRANSACTION_FEE {
            return self.build_err("Fee is less than the minimum");
        }

        // Create transaction outputs


        let mut outputs = match self
            .outputs
            .iter()
            .map(|o| {
                if let Some(rewind_data) = self.rewind_data.as_ref() {
                    // TODO: Should proof be verified?
                    o.as_rewindable_transaction_output(factories, rewind_data, false)
                } else {
                    // TODO: Should proof be verified
                    o.as_transaction_output(factories, false)
                }
            })
            .collect::<Result<Vec<TransactionOutput>, _>>()
        {
            Ok(o) => o,
            Err(e) => {
                return self.build_err(&e.to_string());
            },
        };

        if let Some(change_unblinded_output) = change_output {
            let change_output_script_offset_private_key = match self.change_script_offset_private_key {
                None => return self.build_err("A change output script offset was not provided"),
                Some(ref pk) => pk.clone(),
            };

            self.excess_blinding_factor = self.excess_blinding_factor + change_unblinded_output.spending_key.clone();

            // If rewind data is present we produce a rewindable output, else a standard output
            let change_output = if let Some(rewind_data) = self.rewind_data.as_ref() {
                // TODO: Should proof be verified?
                match change_unblinded_output.as_rewindable_transaction_output(factories, rewind_data, false) {
                    Ok(o) => o,
                    Err(e) => {
                        return self.build_err(e.to_string().as_str());
                    },
                }
            } else {
                // TODO: Should proof be verified?
                match change_unblinded_output.as_transaction_output(factories, false) {
                    Ok(o) => o,
                    Err(e) => {
                        return self.build_err(e.to_string().as_str());
                    },
                }
            };
            self.outputs.push(change_unblinded_output);
            self.script_offset_private_keys
                .push(change_output_script_offset_private_key);
            outputs.push(change_output);
        }

        // Prevent overflow attacks by imposing sane limits on outputs
        if outputs.len() > MAX_TRANSACTION_OUTPUTS {
            return self.build_err("Too many outputs in transaction");
        }

        // Calculate the Inputs portion of Gamma so we don't have to store the individual script private keys in
        // RawTransactionInfo while we wait for the recipients reply
        let mut gamma = PrivateKey::default();
        for uo in self.unblinded_inputs.iter() {
            gamma = gamma + uo.script_private_key.clone();
        }

        if outputs.len() != self.script_offset_private_keys.len() {
            return self
                .build_err("There should be the same number of sender added outputs as script offset private keys");
        }

        for script_offset_pvt_key in self.script_offset_private_keys.iter() {
            gamma = gamma - script_offset_pvt_key.clone();
        }

        let nonce = self.private_nonce.clone().unwrap();
        let public_nonce = PublicKey::from_secret_key(&nonce);
        let offset = self.offset.clone().unwrap();
        let excess_blinding_factor = self.excess_blinding_factor.clone();
        let offset_blinding_factor = &excess_blinding_factor - &offset;
        let excess = PublicKey::from_secret_key(&offset_blinding_factor);
        let amount_to_self = self.outputs.iter().fold(MicroTari::from(0), |sum, o| sum + o.value);

        let recipient_info = match self.num_recipients {
            0 => RecipientInfo::None,
            1 => RecipientInfo::Single(None),
            _ => RecipientInfo::Multiple(HashMap::new()),
        };
        let num_ids = max(1, self.num_recipients);
        let mut ids = Vec::with_capacity(num_ids);
        for i in 0..num_ids {
            ids.push(calculate_tx_id::<D>(&public_nonce, i));
        }

        // The fee should be less than the amount being sent. This isn't a protocol requirement, but it's what you want
        // 99.999% of the time, however, always preventing this will also prevent spending dust in some edge
        // cases.
        if self.amounts.size() > 0 && total_fee > self.calculate_amount_to_others() {
            let ids_clone = ids.to_vec();
            warn!(
                target: LOG_TARGET,
                "Fee ({}) is greater than amount ({}) being sent for Transaction (TxId: {}).",
                total_fee,
                self.calculate_amount_to_others(),
                ids_clone[0]
            );
            if self.prevent_fee_gt_amount {
                return self.build_err("Fee is greater than amount");
            }
        }

        // Everything is here. Let's send some Tari!
        let sender_info = RawTransactionInfo {
            num_recipients: self.num_recipients,
            amount_to_self,
            ids,
            amounts: self.amounts.into_vec(),
            recipient_scripts: self.recipient_scripts.into_vec(),
            recipient_script_offset_private_keys: self.recipient_script_offset_private_keys.into_vec(),
            change,
            change_script_offset_public_key: self
                .change_script_offset_private_key
                .map(|pk| PublicKey::from_secret_key(&pk)),
            metadata: TransactionMetadata {
                fee: total_fee,
                lock_height: self.lock_height.unwrap(),
            },
            inputs: self.inputs,
            outputs,
            offset,
            offset_blinding_factor,
            gamma,
            public_excess: excess,
            private_nonce: nonce,
            public_nonce: public_nonce.clone(),
            public_nonce_sum: public_nonce,
            recipient_info,
            signatures: Vec::new(),
            message: self.message.unwrap_or_else(|| "".to_string()),
            unique_id: self.unique_id
        };

        let state = SenderState::Initializing(Box::new(sender_info));
        let state = state
            .initialize()
            .expect("It should be possible to call initialize from Initializing state");
        Ok(SenderTransactionProtocol { state })
    }
}

//----------------------------------------         Tests          ----------------------------------------------------//

#[cfg(test)]
mod test {
    use crate::{
        consensus::{KERNEL_WEIGHT, WEIGHT_PER_INPUT, WEIGHT_PER_OUTPUT},
        transactions::{
            fee::Fee,
            helpers::{create_test_input, TestParams},
            tari_amount::*,
            transaction::{UnblindedOutput, MAX_TRANSACTION_INPUTS},
            transaction_protocol::{
                sender::SenderState,
                sender_transaction_protocol_builder::SenderTransactionInitializer,
                TransactionProtocolError,
            },
            types::{CryptoFactories, PrivateKey, PublicKey},
        },
    };
    use rand::rngs::OsRng;
    use tari_crypto::{
        common::Blake256,
        inputs,
        keys::{PublicKey as pk, SecretKey},
        script,
        script::{ExecutionStack, TariScript},
    };

    /// One input, 2 outputs
    #[test]
    fn no_receivers() {
        // Create some inputs
        let factories = CryptoFactories::default();
        let p = TestParams::new();
        // Start the builder
        let builder = SenderTransactionInitializer::new(0);
        let err = builder.build::<Blake256>(&factories).unwrap_err();
        let script = script!(Nop);
        // We should have a bunch of fields missing still, but we can recover and continue
        assert_eq!(
            err.message,
            "Missing Lock Height,Missing Fee per gram,Missing Offset,Change script,Change input data,Change script \
             private key"
        );
        let mut builder = err.builder;
        builder
            .with_lock_height(100)
            .with_offset(p.offset)
            .with_private_nonce(p.nonce);
        builder.with_output(
            UnblindedOutput::new(
                MicroTari(100),
                p.spend_key,
                None,
                TariScript::default(),
                ExecutionStack::default(),
                0,
                PrivateKey::default(),
                PublicKey::default(),
            ),
            PrivateKey::random(&mut OsRng),
        );
        let (utxo, input, script_offset) = create_test_input(MicroTari(5_000), 0, 0, &factories.commitment);
        builder.with_input(utxo, input);
        builder
            .with_fee_per_gram(MicroTari(20))
            .with_recipient_script(0, script.clone(), script_offset)
            .with_change_script(script, ExecutionStack::default(), PrivateKey::default());
        let expected_fee = Fee::calculate(MicroTari(20), 1, 1, 2);
        // We needed a change input, so this should fail
        let err = builder.build::<Blake256>(&factories).unwrap_err();
        assert_eq!(err.message, "Change spending key was not provided");
        // Ok, give them a change output
        let mut builder = err.builder;
        builder.with_change_secret(p.change_spend_key);
        let result = builder.build::<Blake256>(&factories).unwrap();
        // Peek inside and check the results
        if let SenderState::Finalizing(info) = result.state {
            assert_eq!(info.num_recipients, 0, "Number of receivers");
            assert_eq!(info.signatures.len(), 0, "Number of signatures");
            assert_eq!(info.ids.len(), 1, "Number of tx_ids");
            assert_eq!(info.amounts.len(), 0, "Number of external payment amounts");
            assert_eq!(info.metadata.lock_height, 100, "Lock height");
            assert_eq!(info.metadata.fee, expected_fee, "Fee");
            assert_eq!(info.outputs.len(), 2, "There should be 2 outputs");
            assert_eq!(info.inputs.len(), 1, "There should be 1 input");
        } else {
            panic!("There were no recipients, so we should be finalizing");
        }
    }

    /// One output, one input
    #[test]
    fn no_change_or_receivers() {
        // Create some inputs
        let factories = CryptoFactories::default();
        let p = TestParams::new();
        let (utxo, input, _) = create_test_input(MicroTari(500), 0, 0, &factories.commitment);
        let expected_fee = Fee::calculate(MicroTari(20), 1, 1, 1);
        let output = UnblindedOutput::new(
            MicroTari(500) - expected_fee,
            p.spend_key,
            None,
            TariScript::default(),
            ExecutionStack::default(),
            0,
            PrivateKey::default(),
            PublicKey::default(),
        );
        // Start the builder
        let mut builder = SenderTransactionInitializer::new(0);
        builder
            .with_lock_height(0)
            .with_offset(p.offset)
            .with_private_nonce(p.nonce)
            .with_output(output, PrivateKey::random(&mut OsRng))
            .with_input(utxo, input)
            .with_fee_per_gram(MicroTari(20))
            .with_prevent_fee_gt_amount(false);
        let result = builder.build::<Blake256>(&factories).unwrap();
        // Peek inside and check the results
        if let SenderState::Finalizing(info) = result.state {
            assert_eq!(info.num_recipients, 0, "Number of receivers");
            assert_eq!(info.signatures.len(), 0, "Number of signatures");
            assert_eq!(info.ids.len(), 1, "Number of tx_ids");
            assert_eq!(info.amounts.len(), 0, "Number of external payment amounts");
            assert_eq!(info.metadata.lock_height, 0, "Lock height");
            assert_eq!(info.metadata.fee, expected_fee, "Fee");
            assert_eq!(info.outputs.len(), 1, "There should be 1 output");
            assert_eq!(info.inputs.len(), 1, "There should be 1 input");
        } else {
            panic!("There were no recipients, so we should be finalizing");
        }
    }

    /// Hit the edge case where our change isn't enough to cover the cost of an extra output
    #[test]
    #[allow(clippy::identity_op)]
    fn change_edge_case() {
        // Create some inputs
        let factories = CryptoFactories::default();
        let p = TestParams::new();
        let (utxo, input, _) = create_test_input(MicroTari(500), 0, 0, &factories.commitment);
        let expected_fee = MicroTari::from((KERNEL_WEIGHT + WEIGHT_PER_INPUT + 1 * WEIGHT_PER_OUTPUT) * 20);
        // fee == 340, output = 80
        // Pay out so that I should get change, but not enough to pay for the output
        let output = UnblindedOutput::new(
            MicroTari(500) - expected_fee - MicroTari(50),
            p.spend_key,
            None,
            TariScript::default(),
            ExecutionStack::default(),
            0,
            PrivateKey::default(),
            PublicKey::default(),
        );
        // Start the builder
        let mut builder = SenderTransactionInitializer::new(0);
        builder
            .with_lock_height(0)
            .with_offset(p.offset)
            .with_private_nonce(p.nonce)
            .with_output(output, PrivateKey::random(&mut OsRng))
            .with_input(utxo, input)
            .with_fee_per_gram(MicroTari(20))
            .with_prevent_fee_gt_amount(false);
        let result = builder.build::<Blake256>(&factories).unwrap();
        // Peek inside and check the results
        if let SenderState::Finalizing(info) = result.state {
            assert_eq!(info.num_recipients, 0, "Number of receivers");
            assert_eq!(info.signatures.len(), 0, "Number of signatures");
            assert_eq!(info.ids.len(), 1, "Number of tx_ids");
            assert_eq!(info.amounts.len(), 0, "Number of external payment amounts");
            assert_eq!(info.metadata.lock_height, 0, "Lock height");
            assert_eq!(info.metadata.fee, expected_fee + MicroTari(50), "Fee");
            assert_eq!(info.outputs.len(), 1, "There should be 1 output");
            assert_eq!(info.inputs.len(), 1, "There should be 1 input");
        } else {
            panic!("There were no recipients, so we should be finalizing");
        }
    }

    #[test]
    fn too_many_inputs() {
        // Create some inputs
        let factories = CryptoFactories::default();
        let p = TestParams::new();
        let output = UnblindedOutput::new(
            MicroTari(500),
            p.spend_key,
            None,
            TariScript::default(),
            ExecutionStack::default(),
            0,
            PrivateKey::default(),
            PublicKey::default(),
        );
        // Start the builder
        let mut builder = SenderTransactionInitializer::new(0);
        builder
            .with_lock_height(0)
            .with_offset(p.offset)
            .with_private_nonce(p.nonce)
            .with_output(output, PrivateKey::random(&mut OsRng))
            .with_fee_per_gram(MicroTari(2));
        for _ in 0..MAX_TRANSACTION_INPUTS + 1 {
            let (utxo, input, _) = create_test_input(MicroTari(50), 0, 0, &factories.commitment);
            builder.with_input(utxo, input);
        }
        let err = builder.build::<Blake256>(&factories).unwrap_err();
        assert_eq!(err.message, "Too many inputs in transaction");
    }

    #[test]
    fn fee_too_low() {
        // Create some inputs
        let factories = CryptoFactories::default();
        let p = TestParams::new();
        let (utxo, input, _) = create_test_input(MicroTari(500), 0, 0, &factories.commitment);
        let script = script!(Nop);
        let script_offset = PrivateKey::default();
        let output = UnblindedOutput::new(
            MicroTari(400),
            p.spend_key,
            None,
            script.clone(),
            inputs!(PublicKey::from_secret_key(&PrivateKey::default())),
            0,
            PrivateKey::default(),
            PublicKey::default(),
        );
        // Start the builder
        let mut builder = SenderTransactionInitializer::new(0);
        builder
            .with_lock_height(0)
            .with_offset(p.offset)
            .with_private_nonce(p.nonce)
            .with_input(utxo, input)
            .with_output(output, PrivateKey::random(&mut OsRng))
            .with_change_secret(p.change_spend_key)
            .with_fee_per_gram(MicroTari(1))
            .with_recipient_script(0, script.clone(), script_offset)
            .with_change_script(script, ExecutionStack::default(), PrivateKey::default());
        let err = builder.build::<Blake256>(&factories).unwrap_err();
        assert_eq!(err.message, "Fee is less than the minimum");
    }

    #[test]
    fn not_enough_funds() {
        // Create some inputs
        let factories = CryptoFactories::default();
        let p = TestParams::new();
        let (utxo, input, _) = create_test_input(MicroTari(400), 0, 0, &factories.commitment);
        let script = script!(Nop);
        let script_offset = PrivateKey::default();
        let output = UnblindedOutput::new(
            MicroTari(400),
            p.spend_key,
            None,
            TariScript::default(),
            ExecutionStack::default(),
            0,
            PrivateKey::default(),
            PublicKey::default(),
        );
        // Start the builder
        let mut builder = SenderTransactionInitializer::new(0);
        builder
            .with_lock_height(0)
            .with_offset(p.offset)
            .with_private_nonce(p.nonce)
            .with_input(utxo, input)
            .with_output(output, PrivateKey::random(&mut OsRng))
            .with_change_secret(p.change_spend_key)
            .with_fee_per_gram(MicroTari(1))
            .with_recipient_script(0, script.clone(), script_offset)
            .with_change_script(script, ExecutionStack::default(), PrivateKey::default());
        let err = builder.build::<Blake256>(&factories).unwrap_err();
        assert_eq!(err.message, "You are spending more than you're providing");
    }

    #[test]
    fn multi_recipients() {
        // Create some inputs
        let factories = CryptoFactories::default();
        let p = TestParams::new();
        let (utxo, input, _) = create_test_input(MicroTari(100_000), 0, 0, &factories.commitment);
        let script = script!(Nop);
        let script_offset = PrivateKey::default();
        let output = UnblindedOutput::new(
            MicroTari(15000),
            p.spend_key,
            None,
            TariScript::default(),
            ExecutionStack::default(),
            0,
            PrivateKey::default(),
            PublicKey::default(),
        );
        // Start the builder
        let mut builder = SenderTransactionInitializer::new(2);
        builder
            .with_lock_height(0)
            .with_offset(p.offset)
            .with_amount(0, MicroTari(1200))
            .with_amount(1, MicroTari(1100))
            .with_private_nonce(p.nonce)
            .with_input(utxo, input)
            .with_output(output, PrivateKey::random(&mut OsRng))
            .with_change_secret(p.change_spend_key)
            .with_fee_per_gram(MicroTari(20))
            .with_recipient_script(0, script.clone(), script_offset.clone())
            .with_recipient_script(1, script.clone(), script_offset)
            .with_change_script(script, ExecutionStack::default(), PrivateKey::default());
        let result = builder.build::<Blake256>(&factories).unwrap();
        // Peek inside and check the results
        if let SenderState::Failed(TransactionProtocolError::UnsupportedError(s)) = result.state {
            assert_eq!(s, "Multiple recipients are not supported yet")
        } else {
            panic!("We should not allow multiple recipients at this time");
        }
    }

    #[test]
    fn single_recipient() {
        // Create some inputs
        let factories = CryptoFactories::default();
        let p = TestParams::new();
        let (utxo1, input1, _) = create_test_input(MicroTari(2000), 0, 0, &factories.commitment);
        let (utxo2, input2, _) = create_test_input(MicroTari(3000), 0, 0, &factories.commitment);
        let weight = MicroTari(30);
        let script = script!(Nop);
        let script_offset = PrivateKey::default();
        let expected_fee = Fee::calculate(weight, 1, 2, 3);
        let output = UnblindedOutput::new(
            MicroTari(1500) - expected_fee,
            p.spend_key,
            None,
            TariScript::default(),
            ExecutionStack::default(),
            0,
            PrivateKey::default(),
            PublicKey::default(),
        );
        // Start the builder
        let mut builder = SenderTransactionInitializer::new(1);
        builder
            .with_lock_height(1234)
            .with_offset(p.offset)
            .with_private_nonce(p.nonce)
            .with_output(output, PrivateKey::random(&mut OsRng))
            .with_input(utxo1, input1)
            .with_input(utxo2, input2)
            .with_amount(0, MicroTari(2500))
            .with_change_secret(p.change_spend_key)
            .with_fee_per_gram(weight)
            .with_recipient_script(0, script.clone(), script_offset)
            .with_change_script(script, ExecutionStack::default(), PrivateKey::default());
        let result = builder.build::<Blake256>(&factories).unwrap();
        // Peek inside and check the results
        if let SenderState::SingleRoundMessageReady(info) = result.state {
            assert_eq!(info.num_recipients, 1, "Number of receivers");
            assert_eq!(info.signatures.len(), 0, "Number of signatures");
            assert_eq!(info.ids.len(), 1, "Number of tx_ids");
            assert_eq!(info.amounts.len(), 1, "Number of external payment amounts");
            assert_eq!(info.metadata.lock_height, 1234, "Lock height");
            assert_eq!(info.metadata.fee, expected_fee, "Fee");
            assert_eq!(info.outputs.len(), 2, "There should be 2 outputs");
            assert_eq!(info.inputs.len(), 2, "There should be 2 input");
        } else {
            panic!("There was a recipient, we should be ready to send a message");
        }
    }

    #[test]
    fn fail_range_proof() {
        // Create some inputs
        let factories = CryptoFactories::new(32);
        let p = TestParams::new();
        let script = script!(Nop);
        let script_offset = PrivateKey::default();
        let (utxo1, input1, _) = create_test_input((2u64.pow(32) + 10000u64).into(), 0, 0, &factories.commitment);
        let weight = MicroTari(30);
        let output = UnblindedOutput::new(
            (1u64.pow(32) + 1u64).into(),
            p.spend_key,
            None,
            TariScript::default(),
            ExecutionStack::default(),
            0,
            PrivateKey::default(),
            PublicKey::default(),
        );
        // Start the builder
        let mut builder = SenderTransactionInitializer::new(1);
        builder
            .with_lock_height(1234)
            .with_offset(p.offset)
            .with_private_nonce(p.nonce)
            .with_output(output, PrivateKey::default())
            .with_input(utxo1, input1)
            .with_amount(0, MicroTari(100))
            .with_change_secret(p.change_spend_key)
            .with_fee_per_gram(weight)
            .with_recipient_script(0, script.clone(), script_offset)
            .with_change_script(script, ExecutionStack::default(), PrivateKey::default());
        let result = builder.build::<Blake256>(&factories);

        match result {
            Ok(_) => panic!("Range proof should have failed to verify"),
            Err(e) => assert!(e.message.contains("Range proof could not be verified")),
        }
    }
}