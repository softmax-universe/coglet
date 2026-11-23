//! Validate the exact R3 rewrite, rather than trusting a custom section or
//! trusting that publication ran a transform. No byte/hash normalization.
use crate::{Error, Result};
use wasmparser::{Operator, OperatorsReader};
pub(super) fn validate(mut ops: OperatorsReader<'_>) -> Result<()> {
    use Operator::*;
    while !ops.eof() {
        let op = ops.read().map_err(|_| Error::UnqualifiedModule)?;
        let width = match op {
            F32Add | F32Sub | F32Mul | F32Div | F32Sqrt | F32Ceil | F32Floor | F32Trunc
            | F32Nearest | F32Min | F32Max | F32DemoteF64 => Some(32),
            F64Add | F64Sub | F64Mul | F64Div | F64Sqrt | F64Ceil | F64Floor | F64Trunc
            | F64Nearest | F64Min | F64Max | F64PromoteF32 => Some(64),
            _ => None,
        };
        if let Some(width) = width {
            let mut next = || ops.read().map_err(|_| Error::UnqualifiedModule);
            let LocalSet { local_index: tmp } = next()? else {
                return Err(Error::UnqualifiedModule);
            };
            if !matches!(next()?,LocalGet {local_index} if local_index==tmp) {
                return Err(Error::UnqualifiedModule);
            }
            match (width, next()?) {
                (32, F32Const { value }) if value.bits() == 0x7fc00000 => {}
                (64, F64Const { value }) if value.bits() == 0x7ff8000000000000 => {}
                _ => return Err(Error::UnqualifiedModule),
            }
            for _ in 0..2 {
                if !matches!(next()?,LocalGet {local_index} if local_index==tmp) {
                    return Err(Error::UnqualifiedModule);
                }
            }
            if !matches!((width, next()?), (32, F32Eq) | (64, F64Eq)) || !matches!(next()?, Select)
            {
                return Err(Error::UnqualifiedModule);
            }
        }
    }
    Ok(())
}
