//! Error types

use num_derive::FromPrimitive;
use num_traits::FromPrimitive;
use thiserror::Error;
use mundis_sdk::decode_error::DecodeError;
use mundis_sdk::instruction::InstructionError;

/// Errors that may be returned by the Token program.
#[derive(Clone, Debug, Eq, Error, FromPrimitive, PartialEq)]
pub enum TokenError {
    // 0
    /// Insufficient funds for the operation requested.
    #[error("Insufficient funds")]
    InsufficientFunds,
    /// Invalid Mint.
    #[error("Invalid Mint")]
    InvalidMint,
    /// Account not associated with this Mint.
    #[error("Account not associated with this Mint")]
    MintMismatch,
    /// Owner does not match.
    #[error("Owner does not match")]
    OwnerMismatch,

    // 5
    /// This token's supply is fixed and new tokens cannot be minted.
    #[error("Fixed supply")]
    FixedSupply,
    /// The account cannot be initialized because it is already being used.
    #[error("Already in use")]
    AlreadyInUse,
    /// Invalid number of provided signers.
    #[error("Invalid number of provided signers")]
    InvalidNumberOfProvidedSigners,
    /// Invalid number of required signers.
    #[error("Invalid number of required signers")]
    InvalidNumberOfRequiredSigners,
    /// State is uninitialized.
    #[error("State is unititialized")]
    UninitializedState,

    // 10
    /// Instruction does not support native tokens
    #[error("Instruction does not support native tokens")]
    NativeNotSupported,
    /// Non-native account can only be closed if its balance is zero
    #[error("Non-native account can only be closed if its balance is zero")]
    NonNativeHasBalance,
    /// Invalid instruction
    #[error("Invalid instruction")]
    InvalidInstruction,
    /// State is invalid for requested operation.
    #[error("State is invalid for requested operation")]
    InvalidState,
    /// Operation overflowed
    #[error("Operation overflowed")]
    Overflow,

    // 15
    /// Account does not support specified authority type.
    #[error("Account does not support specified authority type")]
    AuthorityTypeNotSupported,
    /// This token mint cannot freeze accounts.
    #[error("This token mint cannot freeze accounts")]
    MintCannotFreeze,
    /// Account is frozen; all account operations will fail
    #[error("Account is frozen")]
    AccountFrozen,
    /// Mint decimals mismatch between the client and mint
    #[error("The provided decimals value different from the Mint decimals")]
    MintDecimalsMismatch,
    /// Instruction does not support non-native tokens
    #[error("Instruction does not support non-native tokens")]
    NonNativeNotSupported,
}
impl From<TokenError> for InstructionError {
    fn from(e: TokenError) -> Self {
        InstructionError::Custom(e as u32)
    }
}
impl<T> DecodeError<T> for TokenError {
    fn type_of() -> &'static str {
        "TokenError"
    }
}

pub trait PrintInstructionError {
    fn print<E>(&self)
        where
            E: 'static + std::error::Error + DecodeError<E> + PrintInstructionError + FromPrimitive;
}

impl PrintInstructionError for InstructionError {
    fn print<E>(&self)
        where
            E: 'static + std::error::Error + DecodeError<E> + PrintInstructionError + FromPrimitive,
    {
        match self {
            Self::Custom(error) => {
                if let Some(custom_error) = E::decode_custom_error_to_enum(*error) {
                    custom_error.print::<E>();
                } else {
                    eprintln!("Error: Unknown");
                }
            }
            Self::InvalidArgument => eprintln!("Error: InvalidArgument"),
            Self::InvalidInstructionData => eprintln!("Error: InvalidInstructionData"),
            Self::InvalidAccountData => eprintln!("Error: InvalidAccountData"),
            Self::AccountDataTooSmall => eprintln!("Error: AccountDataTooSmall"),
            Self::InsufficientFunds => eprintln!("Error: InsufficientFunds"),
            Self::IncorrectProgramId => eprintln!("Error: IncorrectProgramId"),
            Self::MissingRequiredSignature => eprintln!("Error: MissingRequiredSignature"),
            Self::AccountAlreadyInitialized => eprintln!("Error: AccountAlreadyInitialized"),
            Self::UninitializedAccount => eprintln!("Error: UninitializedAccount"),
            Self::NotEnoughAccountKeys => eprintln!("Error: NotEnoughAccountKeys"),
            Self::AccountBorrowFailed => eprintln!("Error: AccountBorrowFailed"),
            Self::MaxSeedLengthExceeded => eprintln!("Error: MaxSeedLengthExceeded"),
            Self::InvalidSeeds => eprintln!("Error: InvalidSeeds"),
            Self::BorshIoError(_) => eprintln!("Error: BorshIoError"),
            Self::AccountNotRentExempt => eprintln!("Error: AccountNotRentExempt"),
            Self::UnsupportedSysvar => eprintln!("Error: UnsupportedSysvar"),
            Self::IllegalOwner => eprintln!("Error: IllegalOwner"),
            Self::MaxAccountsDataSizeExceeded => eprintln!("Error: MaxAccountsDataSizeExceeded"),
            Self::ActiveVoteAccountClose => eprintln!("Error: ActiveVoteAccountClose"),
            _ => {}
        }
    }
}