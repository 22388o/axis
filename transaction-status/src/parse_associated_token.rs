use {
    crate::parse_instruction::{
        check_num_accounts, ParsableProgram, ParseInstructionError, ParsedInstructionEnum,
    },
    serde_json::json,
    mundis_sdk::{instruction::CompiledInstruction, pubkey::Pubkey},
};

// A helper function to convert mundis_token_account_program::id() as mundis_sdk::pubkey::Pubkey
// to mundis_sdk::pubkey::Pubkey
pub fn anima_associated_token_id() -> Pubkey {
    Pubkey::new_from_array(mundis_token_account_program::id().to_bytes())
}

pub fn parse_associated_token(
    instruction: &CompiledInstruction,
    account_keys: &[Pubkey],
) -> Result<ParsedInstructionEnum, ParseInstructionError> {
    match instruction.accounts.iter().max() {
        Some(index) if (*index as usize) < account_keys.len() => {}
        _ => {
            // Runtime should prevent this from ever happening
            return Err(ParseInstructionError::InstructionKeyMismatch(
                ParsableProgram::AnimaTokenAccount,
            ));
        }
    }
    check_num_associated_token_accounts(&instruction.accounts, 7)?;
    Ok(ParsedInstructionEnum {
        instruction_type: "create".to_string(),
        info: json!({
            "source": account_keys[instruction.accounts[0] as usize].to_string(),
            "account": account_keys[instruction.accounts[1] as usize].to_string(),
            "wallet": account_keys[instruction.accounts[2] as usize].to_string(),
            "mint": account_keys[instruction.accounts[3] as usize].to_string(),
            "systemProgram": account_keys[instruction.accounts[4] as usize].to_string(),
            "tokenProgram": account_keys[instruction.accounts[5] as usize].to_string(),
            "rentSysvar": account_keys[instruction.accounts[6] as usize].to_string(),
        }),
    })
}

fn check_num_associated_token_accounts(
    accounts: &[u8],
    num: usize,
) -> Result<(), ParseInstructionError> {
    check_num_accounts(accounts, num, ParsableProgram::AnimaTokenAccount)
}

#[cfg(test)]
mod test {
    use {
        super::*,
        anima_token_account::{
            create_associated_token_account,
            mundis_program::{
                instruction::CompiledInstruction as AnimaAssociatedTokenCompiledInstruction,
                message::Message, pubkey::Pubkey as AnimaAssociatedTokenPubkey,
            },
        },
    };

    fn convert_pubkey(pubkey: Pubkey) -> AnimaAssociatedTokenPubkey {
        AnimaAssociatedTokenPubkey::new_from_array(pubkey.to_bytes())
    }

    fn convert_compiled_instruction(
        instruction: &AnimaAssociatedTokenCompiledInstruction,
    ) -> CompiledInstruction {
        CompiledInstruction {
            program_id_index: instruction.program_id_index,
            accounts: instruction.accounts.clone(),
            data: instruction.data.clone(),
        }
    }

    #[test]
    fn test_parse_associated_token() {
        let mut keys: Vec<Pubkey> = vec![];
        for _ in 0..7 {
            keys.push(mundis_sdk::pubkey::new_rand());
        }

        let create_ix = create_associated_token_account(
            &convert_pubkey(keys[0]),
            &convert_pubkey(keys[1]),
            &convert_pubkey(keys[2]),
        );
        let message = Message::new(&[create_ix], None);
        let compiled_instruction = convert_compiled_instruction(&message.instructions[0]);
        assert_eq!(
            parse_associated_token(&compiled_instruction, &keys).unwrap(),
            ParsedInstructionEnum {
                instruction_type: "create".to_string(),
                info: json!({
                    "source": keys[0].to_string(),
                    "account": keys[1].to_string(),
                    "wallet": keys[2].to_string(),
                    "mint": keys[3].to_string(),
                    "systemProgram": keys[4].to_string(),
                    "tokenProgram": keys[5].to_string(),
                    "rentSysvar": keys[6].to_string(),
                })
            }
        );
    }
}
