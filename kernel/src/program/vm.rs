//! Deterministic XPVM bytecode contract, interpreter, and metering.

use crate::program::{ProgramId, ProgramRegistry};

pub const MAGIC: [u8; 4] = *b"XPVM";
pub const VERSION: u8 = 1;
pub const MAX_STACK_ITEMS: u16 = 256;
pub const MAX_MEMORY_PAGES: u16 = 16;
pub const PAGE_BYTES: usize = 65_536;
pub const MAX_CALL_FUEL: u64 = 65_536;
const HEADER_LEN: usize = 13;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValidatedCode {
    pub entry: u32,
    pub max_stack: u16,
    pub memory_pages: u16,
    pub instruction_count: u32,
    pub instruction_fuel: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeError {
    InvalidHeader,
    UnsupportedVersion,
    InvalidLimit,
    InvalidEntry,
    InvalidInstruction,
    MissingReturn,
}

/// Version 1 charges one unit per instruction and one unit per declared memory
/// page. No instruction can access memory or call the host in this version.
pub const INSTRUCTION_COST: u64 = 1;
pub const MEMORY_PAGE_COST: u64 = 1;
pub const STATE_READ_COST: u64 = 2;
pub const STATE_WRITE_COST: u64 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutionResult {
    pub value: i64,
    pub fuel_used: u64,
    pub proposed_effect: Option<VmEffect>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionError {
    InvalidCode(CodeError),
    UnknownProgram,
    OutOfFuel,
    ArithmeticOverflow,
}

/// Resolve deployed code from the canonical registry before interpreting it.
pub fn execute_registered(
    registry: &ProgramRegistry,
    id: ProgramId,
    fuel_limit: u64,
) -> Result<ExecutionResult, ExecutionError> {
    let record = registry
        .program(&id)
        .ok_or(ExecutionError::UnknownProgram)?;
    execute_code_with_state(&record.code, record.state_value, fuel_limit)
}

/// Execute validated version 1 code with an explicit fuel ceiling. The
/// interpreter owns its stack; execution has no mutable ledger access.
pub fn execute_code(code: &[u8], fuel_limit: u64) -> Result<ExecutionResult, ExecutionError> {
    execute_code_with_state(code, 0, fuel_limit)
}

fn execute_code_with_state(
    code: &[u8],
    initial_state: i64,
    fuel_limit: u64,
) -> Result<ExecutionResult, ExecutionError> {
    let validated = validate_code(code).map_err(ExecutionError::InvalidCode)?;
    let memory_cost = u64::from(validated.memory_pages) * MEMORY_PAGE_COST;
    let fuel_used = memory_cost
        .checked_add(validated.instruction_fuel)
        .ok_or(ExecutionError::OutOfFuel)?;
    if fuel_used > fuel_limit {
        return Err(ExecutionError::OutOfFuel);
    }

    let mut stack = Vec::with_capacity(usize::from(validated.max_stack));
    let mut state_value = initial_state;
    let mut proposed_effect = None;
    let mut cursor = HEADER_LEN;
    loop {
        let opcode = code[cursor];
        cursor += 1;
        match opcode {
            0x00 => {}
            0x01 => {
                let value = i64::from_le_bytes(
                    code[cursor..cursor + 8]
                        .try_into()
                        .expect("validated i64 constant"),
                );
                cursor += 8;
                stack.push(value);
            }
            0x02 => {
                let right = stack.pop().expect("validated stack depth");
                let left = stack.pop().expect("validated stack depth");
                stack.push(
                    left.checked_add(right)
                        .ok_or(ExecutionError::ArithmeticOverflow)?,
                );
            }
            0x04 => stack.push(state_value),
            0x05 => {
                state_value = stack.pop().expect("validated state write");
                proposed_effect = Some(VmEffect::ProgramState(state_value));
            }
            0x03 => {
                return Ok(ExecutionResult {
                    value: stack.pop().expect("validated return value"),
                    fuel_used,
                    proposed_effect,
                });
            }
            _ => unreachable!("validated opcode"),
        }
    }
}

/// Format: magic[4], version[1], stack_limit[2 LE], memory_pages[2 LE],
/// entry_offset[4 LE], then opcodes. Instructions: nop(00), i64.const(01+8 LE),
/// i64.add(02), return(03), state.get(04), state.set(05). State access is
/// limited to the program's own fixed-size i64 slot; there are no branches.
pub fn validate_code(code: &[u8]) -> Result<ValidatedCode, CodeError> {
    if code.len() < HEADER_LEN || code[..4] != MAGIC {
        return Err(CodeError::InvalidHeader);
    }
    if code[4] != VERSION {
        return Err(CodeError::UnsupportedVersion);
    }
    let max_stack = u16::from_le_bytes([code[5], code[6]]);
    let memory_pages = u16::from_le_bytes([code[7], code[8]]);
    if max_stack == 0 || max_stack > MAX_STACK_ITEMS || memory_pages > MAX_MEMORY_PAGES {
        return Err(CodeError::InvalidLimit);
    }
    let entry = u32::from_le_bytes(
        code[9..13]
            .try_into()
            .map_err(|_| CodeError::InvalidHeader)?,
    );
    let body = &code[HEADER_LEN..];
    let mut offset = 0usize;
    let mut boundaries = Vec::new();
    let mut depth = 0u16;
    let mut count = 0u32;
    let mut instruction_fuel = 0u64;
    let mut returned = false;
    while offset < body.len() {
        boundaries.push(offset);
        let opcode = body[offset];
        offset += 1;
        count = count.checked_add(1).ok_or(CodeError::InvalidInstruction)?;
        let cost = match opcode {
            0x04 => STATE_READ_COST,
            0x05 => STATE_WRITE_COST,
            _ => INSTRUCTION_COST,
        };
        instruction_fuel = instruction_fuel
            .checked_add(cost)
            .ok_or(CodeError::InvalidLimit)?;
        match opcode {
            0x00 => {}
            0x01 => {
                if body.len().saturating_sub(offset) < 8 {
                    return Err(CodeError::InvalidInstruction);
                }
                offset += 8;
                depth = depth.checked_add(1).ok_or(CodeError::InvalidLimit)?;
                if depth > max_stack {
                    return Err(CodeError::InvalidLimit);
                }
            }
            0x02 => {
                if depth < 2 {
                    return Err(CodeError::InvalidInstruction);
                }
                depth -= 1;
            }
            0x04 => {
                depth = depth.checked_add(1).ok_or(CodeError::InvalidLimit)?;
                if depth > max_stack {
                    return Err(CodeError::InvalidLimit);
                }
            }
            0x05 => {
                if depth == 0 {
                    return Err(CodeError::InvalidInstruction);
                }
                depth -= 1;
            }
            0x03 => {
                if depth != 1 || offset != body.len() {
                    return Err(CodeError::InvalidInstruction);
                }
                returned = true;
            }
            _ => return Err(CodeError::InvalidInstruction),
        }
        if returned {
            break;
        }
    }
    if !returned {
        return Err(CodeError::MissingReturn);
    }
    if entry != 0 || !boundaries.contains(&0) {
        return Err(CodeError::InvalidEntry);
    }
    Ok(ValidatedCode {
        entry,
        max_stack,
        memory_pages,
        instruction_count: count,
        instruction_fuel,
    })
}

/// The VM proposes a change to its own fixed-size state slot. The kernel applies
/// the proposal only after successful execution and journals the old value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VmEffect {
    ProgramState(i64),
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_canonical_bounded_code() {
        let mut code = b"XPVM".to_vec();
        code.extend_from_slice(&[1, 2, 0, 0, 0, 0, 0, 0, 0]);
        code.push(1);
        code.extend_from_slice(&7u64.to_le_bytes());
        code.push(3);
        assert_eq!(validate_code(&code).unwrap().instruction_count, 2);
        code[4] = 2;
        assert_eq!(validate_code(&code), Err(CodeError::UnsupportedVersion));
    }

    #[test]
    fn execution_is_metered_and_deterministic() {
        let mut code = b"XPVM".to_vec();
        code.extend_from_slice(&[1, 2, 0, 1, 0, 0, 0, 0, 0]);
        for value in [7i64, 9] {
            code.push(1);
            code.extend_from_slice(&value.to_le_bytes());
        }
        code.extend_from_slice(&[2, 3]);
        assert_eq!(execute_code(&code, 4), Err(ExecutionError::OutOfFuel));
        assert_eq!(
            execute_code(&code, 5),
            Ok(ExecutionResult {
                value: 16,
                fuel_used: 5,
                proposed_effect: None,
            })
        );
        assert_eq!(execute_code(&code, 5), execute_code(&code, 5));
    }

    #[test]
    fn arithmetic_overflow_traps_without_effects() {
        let mut code = b"XPVM".to_vec();
        code.extend_from_slice(&[1, 2, 0, 0, 0, 0, 0, 0, 0]);
        for value in [i64::MAX, 1] {
            code.push(1);
            code.extend_from_slice(&value.to_le_bytes());
        }
        code.extend_from_slice(&[2, 3]);
        assert_eq!(
            execute_code(&code, 4),
            Err(ExecutionError::ArithmeticOverflow)
        );
    }

    #[test]
    fn registered_code_executes_and_missing_id_fails() {
        use crate::{
            common::Height,
            program::{DeployProgram, deploy_program},
        };
        let mut registry = ProgramRegistry::default();
        let mut code = b"XPVM".to_vec();
        code.extend_from_slice(&[1, 1, 0, 0, 0, 0, 0, 0, 0]);
        code.push(1);
        code.extend_from_slice(&42i64.to_le_bytes());
        code.push(3);
        let (id, _) = deploy_program(
            &mut registry,
            DeployProgram {
                owner: crypto::Address::ZERO,
                nonce: 1,
                code,
            },
            Height(1),
        )
        .unwrap();
        assert_eq!(execute_registered(&registry, id, 2).unwrap().value, 42);
        assert_eq!(
            execute_registered(&registry, ProgramId::from_bytes([0; crypto::HASH_SIZE]), 2),
            Err(ExecutionError::UnknownProgram)
        );
    }

    #[test]
    fn program_state_proposal_is_metered_and_rollback_restores_value() {
        use crate::{
            common::Height,
            ledger::{LedgerState, StateRollbackJournal},
            program::{DeployProgram, ProgramJournal, deploy_program},
        };
        let mut state = LedgerState::default();
        let mut code = b"XPVM".to_vec();
        code.extend_from_slice(&[1, 2, 0, 0, 0, 0, 0, 0, 0]);
        code.push(0x04);
        code.push(0x01);
        code.extend_from_slice(&1i64.to_le_bytes());
        code.extend_from_slice(&[0x02, 0x05, 0x04, 0x03]);
        let (id, _) = deploy_program(
            &mut state.programs,
            DeployProgram {
                owner: crypto::Address::ZERO,
                nonce: 1,
                code,
            },
            Height(1),
        )
        .unwrap();
        assert_eq!(
            execute_registered(&state.programs, id, 11),
            Err(ExecutionError::OutOfFuel)
        );
        let first = execute_registered(&state.programs, id, 12).unwrap();
        assert_eq!(
            (first.value, first.fuel_used, first.proposed_effect),
            (1, 12, Some(VmEffect::ProgramState(1)))
        );
        assert_eq!(state.programs.program(&id).unwrap().state_value, 0);
        let Some(VmEffect::ProgramState(value)) = first.proposed_effect else {
            panic!("missing state proposal")
        };
        let previous = state.programs.set_state(id, value).unwrap();
        let updated = state.clone();
        assert_eq!(
            execute_registered(&state.programs, id, 12)
                .unwrap()
                .proposed_effect,
            Some(VmEffect::ProgramState(2))
        );
        state
            .rollback_state(StateRollbackJournal {
                coin: None,
                program: Some(ProgramJournal::State {
                    program_id: id,
                    previous,
                }),
                extension: None,
            })
            .unwrap();
        assert_eq!(state.programs.program(&id).unwrap().state_value, 0);
        assert_ne!(state, updated);
    }
}
