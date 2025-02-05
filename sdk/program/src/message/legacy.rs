//! The original and current Mundis message format.
//!
//! This crate defines two versions of `Message` in their own modules:
//! [`legacy`] and [`v0`]. `legacy` is the current version as of Solana 1.10.0.
//! `v0` is a [future message format] that encodes more account keys into a
//! transaction than the legacy format.
//!
//! [`legacy`]: crate::message::legacy
//! [`v0`]: crate::message::v0
//! [future message format]: https://docs.solana.com/proposals/transactions-v2

#![allow(clippy::integer_arithmetic)]

use {
    crate::{
        hash::Hash,
        instruction::{AccountMeta, CompiledInstruction, Instruction},
        message::MessageHeader,
        pubkey::Pubkey,
        sanitize::{Sanitize, SanitizeError},
        short_vec, system_instruction, system_program, sysvar, wasm_bindgen,
    },
    lazy_static::lazy_static,
    std::{collections::BTreeSet, convert::TryFrom, str::FromStr},
};

lazy_static! {
    // Copied keys over since direct references create cyclical dependency.
    pub static ref BUILTIN_PROGRAMS_KEYS: [Pubkey; 10] = {
        let parse = |s| Pubkey::from_str(s).unwrap();
        [
            parse("Config1111111111111111111111111111111111111"),
            parse("Feature111111111111111111111111111111111111"),
            parse("NativeLoader1111111111111111111111111111111"),
            parse("Stake11111111111111111111111111111111111111"),
            parse("StakeConfig11111111111111111111111111111111"),
            parse("Vote111111111111111111111111111111111111111"),
            parse("Memo111111111111111111111111111111111111111"),
            parse("Token11111111111111111111111111111111111111"),
            parse("TokenAccount1111111111111111111111111111111"),
            system_program::id(),
        ]
    };
}

fn position(keys: &[Pubkey], key: &Pubkey) -> u8 {
    keys.iter().position(|k| k == key).unwrap() as u8
}

fn compile_instruction(ix: &Instruction, keys: &[Pubkey]) -> CompiledInstruction {
    let accounts: Vec<_> = ix
        .accounts
        .iter()
        .map(|account_meta| position(keys, &account_meta.pubkey))
        .collect();

    CompiledInstruction {
        program_id_index: position(keys, &ix.program_id),
        data: ix.data.clone(),
        accounts,
    }
}

fn compile_instructions(ixs: &[Instruction], keys: &[Pubkey]) -> Vec<CompiledInstruction> {
    ixs.iter().map(|ix| compile_instruction(ix, keys)).collect()
}

/// A helper struct to collect pubkeys referenced by a set of instructions and read-only counts
#[derive(Debug, PartialEq, Eq)]
struct InstructionKeys {
    pub signed_keys: Vec<Pubkey>,
    pub unsigned_keys: Vec<Pubkey>,
    pub num_readonly_signed_accounts: u8,
    pub num_readonly_unsigned_accounts: u8,
}

impl InstructionKeys {
    fn new(
        signed_keys: Vec<Pubkey>,
        unsigned_keys: Vec<Pubkey>,
        num_readonly_signed_accounts: u8,
        num_readonly_unsigned_accounts: u8,
    ) -> Self {
        Self {
            signed_keys,
            unsigned_keys,
            num_readonly_signed_accounts,
            num_readonly_unsigned_accounts,
        }
    }
}

/// Return pubkeys referenced by all instructions, with the ones needing signatures first. If the
/// payer key is provided, it is always placed first in the list of signed keys. Read-only signed
/// accounts are placed last in the set of signed accounts. Read-only unsigned accounts,
/// including program ids, are placed last in the set. No duplicates and order is preserved.
fn get_keys(instructions: &[Instruction], payer: Option<&Pubkey>) -> InstructionKeys {
    let programs: Vec<_> = get_program_ids(instructions)
        .iter()
        .map(|program_id| AccountMeta {
            pubkey: *program_id,
            is_signer: false,
            is_writable: false,
        })
        .collect();
    let mut keys_and_signed: Vec<_> = instructions
        .iter()
        .flat_map(|ix| ix.accounts.iter())
        .collect();
    keys_and_signed.extend(&programs);
    keys_and_signed.sort_by(|x, y| {
        y.is_signer
            .cmp(&x.is_signer)
            .then(y.is_writable.cmp(&x.is_writable))
    });

    let payer_account_meta;
    if let Some(payer) = payer {
        payer_account_meta = AccountMeta {
            pubkey: *payer,
            is_signer: true,
            is_writable: true,
        };
        keys_and_signed.insert(0, &payer_account_meta);
    }

    let mut unique_metas: Vec<AccountMeta> = vec![];
    for account_meta in keys_and_signed {
        // Promote to writable if a later AccountMeta requires it
        if let Some(x) = unique_metas
            .iter_mut()
            .find(|x| x.pubkey == account_meta.pubkey)
        {
            x.is_writable |= account_meta.is_writable;
            continue;
        }
        unique_metas.push(account_meta.clone());
    }

    let mut signed_keys = vec![];
    let mut unsigned_keys = vec![];
    let mut num_readonly_signed_accounts = 0;
    let mut num_readonly_unsigned_accounts = 0;
    for account_meta in unique_metas {
        if account_meta.is_signer {
            signed_keys.push(account_meta.pubkey);
            if !account_meta.is_writable {
                num_readonly_signed_accounts += 1;
            }
        } else {
            unsigned_keys.push(account_meta.pubkey);
            if !account_meta.is_writable {
                num_readonly_unsigned_accounts += 1;
            }
        }
    }
    InstructionKeys::new(
        signed_keys,
        unsigned_keys,
        num_readonly_signed_accounts,
        num_readonly_unsigned_accounts,
    )
}

/// Return program ids referenced by all instructions.  No duplicates and order is preserved.
fn get_program_ids(instructions: &[Instruction]) -> Vec<Pubkey> {
    let mut set = BTreeSet::new();
    instructions
        .iter()
        .map(|ix| ix.program_id)
        .filter(|&program_id| set.insert(program_id))
        .collect()
}

/// A Mundis transaction message (legacy).
///
/// See the [`message`] module documentation for further description.
///
/// [`message`]: crate::message
///
/// Some constructors accept an optional `payer`, the account responsible for
/// paying the cost of executing a transaction. In most cases, callers should
/// specify the payer explicitly in these constructors. In some cases though,
/// the caller is not _required_ to specify the payer, but is still allowed to:
/// in the `Message` structure, the first account is always the fee-payer, so if
/// the caller has knowledge that the first account of the constructed
/// transaction's `Message` is both a signer and the expected fee-payer, then
/// redundantly specifying the fee-payer is not strictly required.
// NOTE: Serialization-related changes must be paired with the custom serialization
// for versioned messages in the `RemainingLegacyMessage` struct.
#[wasm_bindgen]
#[frozen_abi(digest = "2KnLEqfLcTBQqitE22Pp8JYkaqVVbAkGbCfdeHoyxcAU")]
#[derive(Serialize, Deserialize, Default, Debug, PartialEq, Eq, Clone, AbiExample)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    /// The message header, identifying signed and read-only `account_keys`.
    // NOTE: Serialization-related changes must be paired with the direct read at sigverify.
    #[wasm_bindgen(skip)]
    pub header: MessageHeader,

    /// All the account keys used by this transaction.
    #[wasm_bindgen(skip)]
    #[serde(with = "short_vec")]
    pub account_keys: Vec<Pubkey>,

    /// The id of a recent ledger entry.
    pub recent_blockhash: Hash,

    /// Programs that will be executed in sequence and committed in one atomic transaction if all
    /// succeed.
    #[wasm_bindgen(skip)]
    #[serde(with = "short_vec")]
    pub instructions: Vec<CompiledInstruction>,
}

impl Sanitize for Message {
    fn sanitize(&self) -> std::result::Result<(), SanitizeError> {
        // signing area and read-only non-signing area should not overlap
        if self.header.num_required_signatures as usize
            + self.header.num_readonly_unsigned_accounts as usize
            > self.account_keys.len()
        {
            return Err(SanitizeError::IndexOutOfBounds);
        }

        // there should be at least 1 RW fee-payer account.
        if self.header.num_readonly_signed_accounts >= self.header.num_required_signatures {
            return Err(SanitizeError::IndexOutOfBounds);
        }

        for ci in &self.instructions {
            if ci.program_id_index as usize >= self.account_keys.len() {
                return Err(SanitizeError::IndexOutOfBounds);
            }
            // A program cannot be a payer.
            if ci.program_id_index == 0 {
                return Err(SanitizeError::IndexOutOfBounds);
            }
            for ai in &ci.accounts {
                if *ai as usize >= self.account_keys.len() {
                    return Err(SanitizeError::IndexOutOfBounds);
                }
            }
        }
        self.account_keys.sanitize()?;
        self.recent_blockhash.sanitize()?;
        self.instructions.sanitize()?;
        Ok(())
    }
}

impl Message {
    /// Create a new `Message`.
    ///
    /// # Examples
    ///
    /// This example uses the [`solana_sdk`], [`solana_client`] and [`anyhow`] crates.
    ///
    /// [`mundis_sdk`]: https://docs.rs/solana-sdk
    /// [`solana_client`]: https://docs.rs/solana-client
    /// [`anyhow`]: https://docs.rs/anyhow
    ///
    /// ```
    /// # use mundis_program::example_mocks::mundis_sdk;
    /// # use mundis_program::example_mocks::mundis_client;
    /// use anyhow::Result;
    /// use borsh::{BorshSerialize, BorshDeserialize};
    /// use mundis_client::rpc_client::RpcClient;
    /// use mundis_sdk::{
    ///      instruction::Instruction,
    ///      message::Message,
    ///      pubkey::Pubkey,
    ///      signature::Keypair,
    ///      transaction::Transaction,
    /// };
    ///
    /// // A custom program instruction. This would typically be defined in
    /// // another crate so it can be shared between the on-chain program and
    /// // the client.
    /// #[derive(BorshSerialize, BorshDeserialize)]
    /// enum BankInstruction {
    ///     Initialize,
    ///     Deposit { lamports: u64 },
    ///     Withdraw { lamports: u64 },
    /// }
    ///
    /// fn send_initialize_tx(
    ///     client: &RpcClient,
    ///     program_id: Pubkey,
    ///     payer: &Keypair
    /// ) -> Result<()> {
    ///
    ///     let bank_instruction = BankInstruction::Initialize;
    ///
    ///     let instruction = Instruction::new_with_borsh(
    ///         program_id,
    ///         &bank_instruction,
    ///         vec![],
    ///     );
    ///
    ///     let message = Message::new(
    ///         &[instruction],
    ///         Some(&payer.pubkey()),
    ///     );
    ///
    ///     let blockhash = client.get_latest_blockhash()?;
    ///     let mut tx = Transaction::new(&[payer], message, blockhash);
    ///     client.send_and_confirm_transaction(&tx)?;
    ///
    ///     Ok(())
    /// }
    /// #
    /// # let client = RpcClient::new(String::new());
    /// # let program_id = Pubkey::new_unique();
    /// # let payer = Keypair::new();
    /// # send_initialize_tx(&client, program_id, &payer)?;
    /// #
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn new(instructions: &[Instruction], payer: Option<&Pubkey>) -> Self {
        Self::new_with_blockhash(instructions, payer, &Hash::default())
    }

    /// Create a new message while setting the blockhash.
    ///
    /// # Examples
    ///
    /// This example uses the [`solana_sdk`], [`solana_client`] and [`anyhow`] crates.
    ///
    /// [`solana_sdk`]: https://docs.rs/solana-sdk
    /// [`solana_client`]: https://docs.rs/solana-client
    /// [`anyhow`]: https://docs.rs/anyhow
    ///
    /// ```
    /// # use mundis_program::example_mocks::solana_sdk;
    /// # use mundis_program::example_mocks::solana_client;
    /// use anyhow::Result;
    /// use borsh::{BorshSerialize, BorshDeserialize};
    /// use mundis_client::rpc_client::RpcClient;
    /// use mundis_sdk::{
    ///      instruction::Instruction,
    ///      message::Message,
    ///      pubkey::Pubkey,
    ///      signature::Keypair,
    ///      transaction::Transaction,
    /// };
    ///
    /// // A custom program instruction. This would typically be defined in
    /// // another crate so it can be shared between the on-chain program and
    /// // the client.
    /// #[derive(BorshSerialize, BorshDeserialize)]
    /// enum BankInstruction {
    ///     Initialize,
    ///     Deposit { lamports: u64 },
    ///     Withdraw { lamports: u64 },
    /// }
    ///
    /// fn send_initialize_tx(
    ///     client: &RpcClient,
    ///     program_id: Pubkey,
    ///     payer: &Keypair
    /// ) -> Result<()> {
    ///
    ///     let bank_instruction = BankInstruction::Initialize;
    ///
    ///     let instruction = Instruction::new_with_borsh(
    ///         program_id,
    ///         &bank_instruction,
    ///         vec![],
    ///     );
    ///
    ///     let blockhash = client.get_latest_blockhash()?;
    ///
    ///     let message = Message::new_with_blockhash(
    ///         &[instruction],
    ///         Some(&payer.pubkey()),
    ///         &blockhash,
    ///     );
    ///
    ///     let mut tx = Transaction::new_unsigned(message);
    ///     tx.sign(&[payer], tx.message.recent_blockhash);
    ///     client.send_and_confirm_transaction(&tx)?;
    ///
    ///     Ok(())
    /// }
    /// #
    /// # let client = RpcClient::new(String::new());
    /// # let program_id = Pubkey::new_unique();
    /// # let payer = Keypair::new();
    /// # send_initialize_tx(&client, program_id, &payer)?;
    /// #
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn new_with_blockhash(
        instructions: &[Instruction],
        payer: Option<&Pubkey>,
        blockhash: &Hash,
    ) -> Self {
        let InstructionKeys {
            mut signed_keys,
            unsigned_keys,
            num_readonly_signed_accounts,
            num_readonly_unsigned_accounts,
        } = get_keys(instructions, payer);
        let num_required_signatures = signed_keys.len() as u8;
        signed_keys.extend(&unsigned_keys);
        let instructions = compile_instructions(instructions, &signed_keys);
        Self::new_with_compiled_instructions(
            num_required_signatures,
            num_readonly_signed_accounts,
            num_readonly_unsigned_accounts,
            signed_keys,
            *blockhash,
            instructions,
        )
    }

    /// Create a new message for a [nonced transaction].
    ///
    /// [nonced transaction]: https://docs.solana.com/implemented-proposals/durable-tx-nonces
    ///
    /// In this type of transaction, the blockhash is replaced with a _durable
    /// transaction nonce_, allowing for extended time to pass between the
    /// transaction's signing and submission to the blockchain.
    ///
    /// # Examples
    ///
    /// This example uses the [`mundis_sdk`], [`mundis_client`] and [`anyhow`] crates.
    ///
    /// [`mundis_sdk`]: https://docs.rs/mundis-sdk
    /// [`mundis_client`]: https://docs.rs/mundis-client
    /// [`anyhow`]: https://docs.rs/anyhow
    ///
    /// ```
    /// # use mundis_program::example_mocks::mundis_sdk;
    /// # use mundis_program::example_mocks::mundis_client;
    /// use anyhow::Result;
    /// use borsh::{BorshSerialize, BorshDeserialize};
    /// use mundis_client::rpc_client::RpcClient;
    /// use mundis_sdk::{
    ///      hash::Hash,
    ///      instruction::Instruction,
    ///      message::Message,
    ///      nonce,
    ///      pubkey::Pubkey,
    ///      signature::Keypair,
    ///      system_instruction,
    ///      transaction::Transaction,
    /// };
    ///
    /// // A custom program instruction. This would typically be defined in
    /// // another crate so it can be shared between the on-chain program and
    /// // the client.
    /// #[derive(BorshSerialize, BorshDeserialize)]
    /// enum BankInstruction {
    ///     Initialize,
    ///     Deposit { lamports: u64 },
    ///     Withdraw { lamports: u64 },
    /// }
    ///
    /// // Create a nonced transaction for later signing and submission,
    /// // returning it and the nonce account's pubkey.
    /// fn create_offline_initialize_tx(
    ///     client: &RpcClient,
    ///     program_id: Pubkey,
    ///     payer: &Keypair
    /// ) -> Result<(Transaction, Pubkey)> {
    ///
    ///     let bank_instruction = BankInstruction::Initialize;
    ///     let bank_instruction = Instruction::new_with_borsh(
    ///         program_id,
    ///         &bank_instruction,
    ///         vec![],
    ///     );
    ///
    ///     // This will create a nonce account and assign authority to the
    ///     // payer so they can sign to advance the nonce and withdraw its rent.
    ///     let nonce_account = make_nonce_account(client, payer)?;
    ///
    ///     let mut message = Message::new_with_nonce(
    ///         vec![bank_instruction],
    ///         Some(&payer.pubkey()),
    ///         &nonce_account,
    ///         &payer.pubkey()
    ///     );
    ///
    ///     // This transaction will need to be signed later, using the blockhash
    ///     // stored in the nonce account.
    ///     let tx = Transaction::new_unsigned(message);
    ///
    ///     Ok((tx, nonce_account))
    /// }
    ///
    /// fn make_nonce_account(client: &RpcClient, payer: &Keypair)
    ///     -> Result<Pubkey>
    /// {
    ///     let nonce_account_address = Keypair::new();
    ///     let nonce_account_size = nonce::State::size();
    ///     let nonce_rent = client.get_minimum_balance_for_rent_exemption(nonce_account_size)?;
    ///
    ///     // Assigning the nonce authority to the payer so they can sign for the withdrawal,
    ///     // and we can throw away the nonce address secret key.
    ///     let create_nonce_instr = system_instruction::create_nonce_account(
    ///         &payer.pubkey(),
    ///         &nonce_account_address.pubkey(),
    ///         &payer.pubkey(),
    ///         nonce_rent,
    ///     );
    ///
    ///    let mut nonce_tx = Transaction::new_with_payer(&create_nonce_instr, Some(&payer.pubkey()));
    ///    let blockhash = client.get_latest_blockhash()?;
    ///    nonce_tx.sign(&[&payer, &nonce_account_address], blockhash);
    ///    client.send_and_confirm_transaction(&nonce_tx)?;
    ///
    ///    Ok(nonce_account_address.pubkey())
    /// }
    /// #
    /// # let client = RpcClient::new(String::new());
    /// # let program_id = Pubkey::new_unique();
    /// # let payer = Keypair::new();
    /// # create_offline_initialize_tx(&client, program_id, &payer)?;
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn new_with_nonce(
        mut instructions: Vec<Instruction>,
        payer: Option<&Pubkey>,
        nonce_account_pubkey: &Pubkey,
        nonce_authority_pubkey: &Pubkey,
    ) -> Self {
        let nonce_ix =
            system_instruction::advance_nonce_account(nonce_account_pubkey, nonce_authority_pubkey);
        instructions.insert(0, nonce_ix);
        Self::new(&instructions, payer)
    }

    pub fn new_with_compiled_instructions(
        num_required_signatures: u8,
        num_readonly_signed_accounts: u8,
        num_readonly_unsigned_accounts: u8,
        account_keys: Vec<Pubkey>,
        recent_blockhash: Hash,
        instructions: Vec<CompiledInstruction>,
    ) -> Self {
        Self {
            header: MessageHeader {
                num_required_signatures,
                num_readonly_signed_accounts,
                num_readonly_unsigned_accounts,
            },
            account_keys,
            recent_blockhash,
            instructions,
        }
    }

    /// Compute the blake3 hash of this transaction's message.
    #[cfg(not(target_arch = "bpf"))]
    pub fn hash(&self) -> Hash {
        let message_bytes = self.serialize();
        Self::hash_raw_message(&message_bytes)
    }

    /// Compute the blake3 hash of a raw transaction message.
    #[cfg(not(target_arch = "bpf"))]
    pub fn hash_raw_message(message_bytes: &[u8]) -> Hash {
        use blake3::traits::digest::Digest;
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"mundis-tx-message-v1");
        hasher.update(message_bytes);
        Hash(<[u8; crate::hash::HASH_BYTES]>::try_from(hasher.finalize().as_slice()).unwrap())
    }

    pub fn compile_instruction(&self, ix: &Instruction) -> CompiledInstruction {
        compile_instruction(ix, &self.account_keys)
    }

    pub fn serialize(&self) -> Vec<u8> {
        bincode::serialize(self).unwrap()
    }

    pub fn program_id(&self, instruction_index: usize) -> Option<&Pubkey> {
        Some(
            &self.account_keys[self.instructions.get(instruction_index)?.program_id_index as usize],
        )
    }

    pub fn program_index(&self, instruction_index: usize) -> Option<usize> {
        Some(self.instructions.get(instruction_index)?.program_id_index as usize)
    }

    pub fn program_ids(&self) -> Vec<&Pubkey> {
        self.instructions
            .iter()
            .map(|ix| &self.account_keys[ix.program_id_index as usize])
            .collect()
    }

    pub fn is_key_passed_to_program(&self, key_index: usize) -> bool {
        if let Ok(key_index) = u8::try_from(key_index) {
            self.instructions
                .iter()
                .any(|ix| ix.accounts.contains(&key_index))
        } else {
            false
        }
    }

    pub fn is_key_called_as_program(&self, key_index: usize) -> bool {
        if let Ok(key_index) = u8::try_from(key_index) {
            self.instructions
                .iter()
                .any(|ix| ix.program_id_index == key_index)
        } else {
            false
        }
    }

    pub fn is_non_loader_key(&self, key_index: usize) -> bool {
        !self.is_key_called_as_program(key_index) || self.is_key_passed_to_program(key_index)
    }

    pub fn program_position(&self, index: usize) -> Option<usize> {
        let program_ids = self.program_ids();
        program_ids
            .iter()
            .position(|&&pubkey| pubkey == self.account_keys[index])
    }

    pub fn maybe_executable(&self, i: usize) -> bool {
        self.program_position(i).is_some()
    }

    pub fn is_writable(&self, i: usize) -> bool {
        let demote_program_id = self.is_key_called_as_program(i);
        (i < (self.header.num_required_signatures - self.header.num_readonly_signed_accounts)
            as usize
            || (i >= self.header.num_required_signatures as usize
                && i < self.account_keys.len()
                    - self.header.num_readonly_unsigned_accounts as usize))
            && !{
                let key = self.account_keys[i];
                sysvar::is_sysvar_id(&key) || BUILTIN_PROGRAMS_KEYS.contains(&key)
            }
            && !demote_program_id
    }

    pub fn is_signer(&self, i: usize) -> bool {
        i < self.header.num_required_signatures as usize
    }

    #[deprecated]
    pub fn get_account_keys_by_lock_type(&self) -> (Vec<&Pubkey>, Vec<&Pubkey>) {
        let mut writable_keys = vec![];
        let mut readonly_keys = vec![];
        for (i, key) in self.account_keys.iter().enumerate() {
            if self.is_writable(i) {
                writable_keys.push(key);
            } else {
                readonly_keys.push(key);
            }
        }
        (writable_keys, readonly_keys)
    }

    #[deprecated]
    pub fn deserialize_instruction(
        index: usize,
        data: &[u8],
    ) -> Result<Instruction, SanitizeError> {
        #[allow(deprecated)]
        sysvar::instructions::load_instruction_at(index, data)
    }

    pub fn signer_keys(&self) -> Vec<&Pubkey> {
        // Clamp in case we're working on un-`sanitize()`ed input
        let last_key = self
            .account_keys
            .len()
            .min(self.header.num_required_signatures as usize);
        self.account_keys[..last_key].iter().collect()
    }

    /// Returns `true` if `account_keys` has any duplicate keys.
    pub fn has_duplicates(&self) -> bool {
        // Note: This is an O(n^2) algorithm, but requires no heap allocations. The benchmark
        // `bench_has_duplicates` in benches/message_processor.rs shows that this implementation is
        // ~50 times faster than using HashSet for very short slices.
        for i in 1..self.account_keys.len() {
            #[allow(clippy::integer_arithmetic)]
            if self.account_keys[i..].contains(&self.account_keys[i - 1]) {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    #![allow(deprecated)]
    use {
        super::*,
        crate::{hash, instruction::AccountMeta, message::MESSAGE_HEADER_LENGTH},
        std::collections::HashSet,
    };

    #[test]
    fn test_message_unique_program_ids() {
        let program_id0 = Pubkey::default();
        let program_ids = get_program_ids(&[
            Instruction::new_with_bincode(program_id0, &0, vec![]),
            Instruction::new_with_bincode(program_id0, &0, vec![]),
        ]);
        assert_eq!(program_ids, vec![program_id0]);
    }

    #[test]
    fn test_builtin_program_keys() {
        let keys: HashSet<Pubkey> = BUILTIN_PROGRAMS_KEYS.iter().copied().collect();
        assert_eq!(keys.len(), 10);
        for k in keys {
            let k = format!("{}", k);
            assert!(k.ends_with("11111111111111111111111"));
        }
    }

    #[test]
    fn test_builtin_program_keys_abi_freeze() {
        // Once the feature is flipped on, we can't further modify
        // BUILTIN_PROGRAMS_KEYS without the risk of breaking consensus.
        let builtins = format!("{:?}", *BUILTIN_PROGRAMS_KEYS);
        assert_eq!(
            format!("{}", hash::hash(builtins.as_bytes())),
            "ACqmMkYbo9eqK6QrRSrB3HLyR6uHhLf31SCfGUAJjiWj"
        );
    }

    #[test]
    fn test_message_unique_program_ids_not_adjacent() {
        let program_id0 = Pubkey::default();
        let program_id1 = Pubkey::new_unique();
        let program_ids = get_program_ids(&[
            Instruction::new_with_bincode(program_id0, &0, vec![]),
            Instruction::new_with_bincode(program_id1, &0, vec![]),
            Instruction::new_with_bincode(program_id0, &0, vec![]),
        ]);
        assert_eq!(program_ids, vec![program_id0, program_id1]);
    }

    #[test]
    fn test_message_unique_program_ids_order_preserved() {
        let program_id0 = Pubkey::new_unique();
        let program_id1 = Pubkey::default(); // Key less than program_id0
        let program_ids = get_program_ids(&[
            Instruction::new_with_bincode(program_id0, &0, vec![]),
            Instruction::new_with_bincode(program_id1, &0, vec![]),
            Instruction::new_with_bincode(program_id0, &0, vec![]),
        ]);
        assert_eq!(program_ids, vec![program_id0, program_id1]);
    }

    #[test]
    fn test_message_unique_keys_both_signed() {
        let program_id = Pubkey::default();
        let id0 = Pubkey::default();
        let keys = get_keys(
            &[
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, true)]),
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, true)]),
            ],
            None,
        );
        assert_eq!(keys, InstructionKeys::new(vec![id0], vec![], 0, 0));
    }

    #[test]
    fn test_message_unique_keys_signed_and_payer() {
        let program_id = Pubkey::default();
        let id0 = Pubkey::default();
        let keys = get_keys(
            &[Instruction::new_with_bincode(
                program_id,
                &0,
                vec![AccountMeta::new(id0, true)],
            )],
            Some(&id0),
        );
        assert_eq!(keys, InstructionKeys::new(vec![id0], vec![], 0, 0));
    }

    #[test]
    fn test_message_unique_keys_unsigned_and_payer() {
        let program_id = Pubkey::default();
        let id0 = Pubkey::default();
        let keys = get_keys(
            &[Instruction::new_with_bincode(
                program_id,
                &0,
                vec![AccountMeta::new(id0, false)],
            )],
            Some(&id0),
        );
        assert_eq!(keys, InstructionKeys::new(vec![id0], vec![], 0, 0));
    }

    #[test]
    fn test_message_unique_keys_one_signed() {
        let program_id = Pubkey::default();
        let id0 = Pubkey::default();
        let keys = get_keys(
            &[
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, false)]),
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, true)]),
            ],
            None,
        );
        assert_eq!(keys, InstructionKeys::new(vec![id0], vec![], 0, 0));
    }

    #[test]
    fn test_message_unique_keys_one_readonly_signed() {
        let program_id = Pubkey::default();
        let id0 = Pubkey::default();
        let keys = get_keys(
            &[
                Instruction::new_with_bincode(
                    program_id,
                    &0,
                    vec![AccountMeta::new_readonly(id0, true)],
                ),
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, true)]),
            ],
            None,
        );

        // Ensure the key is no longer readonly
        assert_eq!(keys, InstructionKeys::new(vec![id0], vec![], 0, 0));
    }

    #[test]
    fn test_message_unique_keys_one_readonly_unsigned() {
        let program_id = Pubkey::default();
        let id0 = Pubkey::default();
        let keys = get_keys(
            &[
                Instruction::new_with_bincode(
                    program_id,
                    &0,
                    vec![AccountMeta::new_readonly(id0, false)],
                ),
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, false)]),
            ],
            None,
        );

        // Ensure the key is no longer readonly
        assert_eq!(keys, InstructionKeys::new(vec![], vec![id0], 0, 0));
    }

    #[test]
    fn test_message_unique_keys_order_preserved() {
        let program_id = Pubkey::default();
        let id0 = Pubkey::new_unique();
        let id1 = Pubkey::default(); // Key less than id0
        let keys = get_keys(
            &[
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, false)]),
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id1, false)]),
            ],
            None,
        );
        assert_eq!(keys, InstructionKeys::new(vec![], vec![id0, id1], 0, 0));
    }

    #[test]
    fn test_message_unique_keys_not_adjacent() {
        let program_id = Pubkey::default();
        let id0 = Pubkey::default();
        let id1 = Pubkey::new_unique();
        let keys = get_keys(
            &[
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, false)]),
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id1, false)]),
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, true)]),
            ],
            None,
        );
        assert_eq!(keys, InstructionKeys::new(vec![id0], vec![id1], 0, 0));
    }

    #[test]
    fn test_message_signed_keys_first() {
        let program_id = Pubkey::default();
        let id0 = Pubkey::default();
        let id1 = Pubkey::new_unique();
        let keys = get_keys(
            &[
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, false)]),
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id1, true)]),
            ],
            None,
        );
        assert_eq!(keys, InstructionKeys::new(vec![id1], vec![id0], 0, 0));
    }

    #[test]
    // Ensure there's a way to calculate the number of required signatures.
    fn test_message_signed_keys_len() {
        let program_id = Pubkey::default();
        let id0 = Pubkey::default();
        let ix = Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, false)]);
        let message = Message::new(&[ix], None);
        assert_eq!(message.header.num_required_signatures, 0);

        let ix = Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, true)]);
        let message = Message::new(&[ix], Some(&id0));
        assert_eq!(message.header.num_required_signatures, 1);
    }

    #[test]
    fn test_message_readonly_keys_last() {
        let program_id = Pubkey::default();
        let id0 = Pubkey::default(); // Identical key/program_id should be de-duped
        let id1 = Pubkey::new_unique();
        let id2 = Pubkey::new_unique();
        let id3 = Pubkey::new_unique();
        let keys = get_keys(
            &[
                Instruction::new_with_bincode(
                    program_id,
                    &0,
                    vec![AccountMeta::new_readonly(id0, false)],
                ),
                Instruction::new_with_bincode(
                    program_id,
                    &0,
                    vec![AccountMeta::new_readonly(id1, true)],
                ),
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id2, false)]),
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id3, true)]),
            ],
            None,
        );
        assert_eq!(
            keys,
            InstructionKeys::new(vec![id3, id1], vec![id2, id0], 1, 1)
        );
    }

    #[test]
    fn test_message_kitchen_sink() {
        let program_id0 = Pubkey::new_unique();
        let program_id1 = Pubkey::new_unique();
        let id0 = Pubkey::default();
        let id1 = Pubkey::new_unique();
        let message = Message::new(
            &[
                Instruction::new_with_bincode(program_id0, &0, vec![AccountMeta::new(id0, false)]),
                Instruction::new_with_bincode(program_id1, &0, vec![AccountMeta::new(id1, true)]),
                Instruction::new_with_bincode(program_id0, &0, vec![AccountMeta::new(id1, false)]),
            ],
            Some(&id1),
        );
        assert_eq!(
            message.instructions[0],
            CompiledInstruction::new(2, &0, vec![1])
        );
        assert_eq!(
            message.instructions[1],
            CompiledInstruction::new(3, &0, vec![0])
        );
        assert_eq!(
            message.instructions[2],
            CompiledInstruction::new(2, &0, vec![0])
        );
    }

    #[test]
    fn test_message_payer_first() {
        let program_id = Pubkey::default();
        let payer = Pubkey::new_unique();
        let id0 = Pubkey::default();

        let ix = Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, false)]);
        let message = Message::new(&[ix], Some(&payer));
        assert_eq!(message.header.num_required_signatures, 1);

        let ix = Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, true)]);
        let message = Message::new(&[ix], Some(&payer));
        assert_eq!(message.header.num_required_signatures, 2);

        let ix = Instruction::new_with_bincode(
            program_id,
            &0,
            vec![AccountMeta::new(payer, true), AccountMeta::new(id0, true)],
        );
        let message = Message::new(&[ix], Some(&payer));
        assert_eq!(message.header.num_required_signatures, 2);
    }

    #[test]
    fn test_message_program_last() {
        let program_id = Pubkey::default();
        let id0 = Pubkey::new_unique();
        let id1 = Pubkey::new_unique();
        let keys = get_keys(
            &[
                Instruction::new_with_bincode(
                    program_id,
                    &0,
                    vec![AccountMeta::new_readonly(id0, false)],
                ),
                Instruction::new_with_bincode(
                    program_id,
                    &0,
                    vec![AccountMeta::new_readonly(id1, true)],
                ),
            ],
            None,
        );
        assert_eq!(
            keys,
            InstructionKeys::new(vec![id1], vec![id0, program_id], 1, 2)
        );
    }

    #[test]
    fn test_program_position() {
        let program_id0 = Pubkey::default();
        let program_id1 = Pubkey::new_unique();
        let id = Pubkey::new_unique();
        let message = Message::new(
            &[
                Instruction::new_with_bincode(program_id0, &0, vec![AccountMeta::new(id, false)]),
                Instruction::new_with_bincode(program_id1, &0, vec![AccountMeta::new(id, true)]),
            ],
            Some(&id),
        );
        assert_eq!(message.program_position(0), None);
        assert_eq!(message.program_position(1), Some(0));
        assert_eq!(message.program_position(2), Some(1));
    }

    #[test]
    fn test_is_writable() {
        let key0 = Pubkey::new_unique();
        let key1 = Pubkey::new_unique();
        let key2 = Pubkey::new_unique();
        let key3 = Pubkey::new_unique();
        let key4 = Pubkey::new_unique();
        let key5 = Pubkey::new_unique();

        let message = Message {
            header: MessageHeader {
                num_required_signatures: 3,
                num_readonly_signed_accounts: 2,
                num_readonly_unsigned_accounts: 1,
            },
            account_keys: vec![key0, key1, key2, key3, key4, key5],
            recent_blockhash: Hash::default(),
            instructions: vec![],
        };
        assert!(message.is_writable(0));
        assert!(!message.is_writable(1));
        assert!(!message.is_writable(2));
        assert!(message.is_writable(3));
        assert!(message.is_writable(4));
        assert!(!message.is_writable(5));
    }

    #[test]
    fn test_get_account_keys_by_lock_type() {
        let program_id = Pubkey::default();
        let id0 = Pubkey::new_unique();
        let id1 = Pubkey::new_unique();
        let id2 = Pubkey::new_unique();
        let id3 = Pubkey::new_unique();
        let message = Message::new(
            &[
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id0, false)]),
                Instruction::new_with_bincode(program_id, &0, vec![AccountMeta::new(id1, true)]),
                Instruction::new_with_bincode(
                    program_id,
                    &0,
                    vec![AccountMeta::new_readonly(id2, false)],
                ),
                Instruction::new_with_bincode(
                    program_id,
                    &0,
                    vec![AccountMeta::new_readonly(id3, true)],
                ),
            ],
            Some(&id1),
        );
        assert_eq!(
            message.get_account_keys_by_lock_type(),
            (vec![&id1, &id0], vec![&id3, &id2, &program_id])
        );
    }

    #[test]
    fn test_program_ids() {
        let key0 = Pubkey::new_unique();
        let key1 = Pubkey::new_unique();
        let loader2 = Pubkey::new_unique();
        let instructions = vec![CompiledInstruction::new(2, &(), vec![0, 1])];
        let message = Message::new_with_compiled_instructions(
            1,
            0,
            2,
            vec![key0, key1, loader2],
            Hash::default(),
            instructions,
        );
        assert_eq!(message.program_ids(), vec![&loader2]);
    }

    #[test]
    fn test_is_key_passed_to_program() {
        let key0 = Pubkey::new_unique();
        let key1 = Pubkey::new_unique();
        let loader2 = Pubkey::new_unique();
        let instructions = vec![CompiledInstruction::new(2, &(), vec![0, 1])];
        let message = Message::new_with_compiled_instructions(
            1,
            0,
            2,
            vec![key0, key1, loader2],
            Hash::default(),
            instructions,
        );

        assert!(message.is_key_passed_to_program(0));
        assert!(message.is_key_passed_to_program(1));
        assert!(!message.is_key_passed_to_program(2));
    }

    #[test]
    fn test_is_non_loader_key() {
        let key0 = Pubkey::new_unique();
        let key1 = Pubkey::new_unique();
        let loader2 = Pubkey::new_unique();
        let instructions = vec![CompiledInstruction::new(2, &(), vec![0, 1])];
        let message = Message::new_with_compiled_instructions(
            1,
            0,
            2,
            vec![key0, key1, loader2],
            Hash::default(),
            instructions,
        );
        assert!(message.is_non_loader_key(0));
        assert!(message.is_non_loader_key(1));
        assert!(!message.is_non_loader_key(2));
    }

    #[test]
    fn test_message_header_len_constant() {
        assert_eq!(
            bincode::serialized_size(&MessageHeader::default()).unwrap() as usize,
            MESSAGE_HEADER_LENGTH
        );
    }

    #[test]
    fn test_message_hash() {
        // when this test fails, it's most likely due to a new serialized format of a message.
        // in this case, the domain prefix `mundis-tx-message-v1` should be updated.
        let program_id0 = Pubkey::from_str("4uQeVj5tqViQh7yWWGStvkEG1Zmhx6uasJtWCJziofM").unwrap();
        let program_id1 = Pubkey::from_str("8opHzTAnfzRpPEx21XtnrVTX28YQuCpAjcn1PczScKh").unwrap();
        let id0 = Pubkey::from_str("CiDwVBFgWV9E5MvXWoLgnEgn2hK7rJikbvfWavzAQz3").unwrap();
        let id1 = Pubkey::from_str("GcdayuLaLyrdmUu324nahyv33G5poQdLUEZ1nEytDeP").unwrap();
        let id2 = Pubkey::from_str("LX3EUdRUBUa3TbsYXLEUdj9J3prXkWXvLYSWyYyc2Jj").unwrap();
        let id3 = Pubkey::from_str("QRSsyMWN1yHT9ir42bgNZUNZ4PdEhcSWCrL2AryKpy5").unwrap();
        let instructions = vec![
            Instruction::new_with_bincode(program_id0, &0, vec![AccountMeta::new(id0, false)]),
            Instruction::new_with_bincode(program_id0, &0, vec![AccountMeta::new(id1, true)]),
            Instruction::new_with_bincode(
                program_id1,
                &0,
                vec![AccountMeta::new_readonly(id2, false)],
            ),
            Instruction::new_with_bincode(
                program_id1,
                &0,
                vec![AccountMeta::new_readonly(id3, true)],
            ),
        ];

        let message = Message::new(&instructions, Some(&id1));
        assert_eq!(
            message.hash(),
            Hash::from_str("CXRH7GHLieaQZRUjH1mpnNnUZQtU4V4RpJpAFgy77i3z").unwrap()
        )
    }
}
