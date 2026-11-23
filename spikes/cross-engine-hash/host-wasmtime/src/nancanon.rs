//! R3 as a module transform: after every float operator whose NaN result payload is
//! implementation-defined by the Wasm spec, replace a NaN result with the canonical NaN.
//!
//!   f32: add sub mul div sqrt ceil floor trunc nearest min max demote(f64)
//!   f64: add sub mul div sqrt ceil floor trunc nearest min max promote(f32)
//!
//! Operators with deterministic NaN handling are left alone: neg, abs, copysign, load/store,
//! reinterpret, const, select, and comparisons. Fixed-width SIMD is not supported (rule R5
//! rejects it), so there is nothing to rewrite there.
//!
//! The rewrite is `local.set $t; (select (local.get $t) (NaN) (f.eq $t $t))`, which is plain core
//! Wasm and therefore behaves the same on every engine, JIT, AOT or interpreter.
use anyhow::Result;
use wasm_encoder::reencode::{Error, Reencode, RoundtripReencoder};
use wasm_encoder::{CodeSection, Function, Ieee32, Ieee64, Instruction as I, ValType};
use wasmparser::{FunctionBody, Operator as O, Parser, Payload};

struct Canon {
    nparams: Vec<u32>, // per defined function
    next: usize,
    inserted: usize,
}

impl Reencode for Canon {
    type Error = std::convert::Infallible;

    fn parse_function_body(&mut self, code: &mut CodeSection, func: FunctionBody<'_>) -> Result<(), Error<Self::Error>> {
        let mut n = self.nparams[self.next];
        self.next += 1;
        let mut locals = vec![];
        for l in func.get_locals_reader()? {
            let (c, t) = l?;
            n += c;
            locals.push((c, self.val_type(t)?));
        }
        let (t32, t64) = (n, n + 1);
        locals.push((1, ValType::F32));
        locals.push((1, ValType::F64));
        let mut f = Function::new(locals);
        for op in func.get_operators_reader()? {
            let op = op?;
            let kind = match &op {
                O::F32Add | O::F32Sub | O::F32Mul | O::F32Div | O::F32Sqrt | O::F32Ceil | O::F32Floor
                | O::F32Trunc | O::F32Nearest | O::F32Min | O::F32Max | O::F32DemoteF64 => Some(32),
                O::F64Add | O::F64Sub | O::F64Mul | O::F64Div | O::F64Sqrt | O::F64Ceil | O::F64Floor
                | O::F64Trunc | O::F64Nearest | O::F64Min | O::F64Max | O::F64PromoteF32 => Some(64),
                _ => None,
            };
            f.instruction(&self.instruction(op)?);
            match kind {
                Some(32) => {
                    self.inserted += 1;
                    f.instruction(&I::LocalSet(t32));
                    f.instruction(&I::LocalGet(t32));
                    f.instruction(&I::F32Const(Ieee32::from(f32::from_bits(0x7fc0_0000))));
                    f.instruction(&I::LocalGet(t32));
                    f.instruction(&I::LocalGet(t32));
                    f.instruction(&I::F32Eq);
                    f.instruction(&I::Select);
                }
                Some(_) => {
                    self.inserted += 1;
                    f.instruction(&I::LocalSet(t64));
                    f.instruction(&I::LocalGet(t64));
                    f.instruction(&I::F64Const(Ieee64::from(f64::from_bits(0x7ff8_0000_0000_0000))));
                    f.instruction(&I::LocalGet(t64));
                    f.instruction(&I::LocalGet(t64));
                    f.instruction(&I::F64Eq);
                    f.instruction(&I::Select);
                }
                None => {}
            }
        }
        code.function(&f);
        Ok(())
    }
}

pub fn canonicalize(bytes: &[u8]) -> Result<(Vec<u8>, usize)> {
    // Pass 1: parameter count of every defined function.
    let mut types: Vec<u32> = vec![];
    let mut fn_types: Vec<u32> = vec![];
    for p in Parser::new(0).parse_all(bytes) {
        match p? {
            Payload::TypeSection(r) => {
                for g in r {
                    for st in g?.types() {
                        types.push(match &st.composite_type.inner {
                            wasmparser::CompositeInnerType::Func(f) => f.params().len() as u32,
                            _ => 0,
                        });
                    }
                }
            }
            Payload::FunctionSection(r) => {
                for t in r {
                    fn_types.push(t?);
                }
            }
            _ => {}
        }
    }
    let nparams = fn_types.iter().map(|t| types[*t as usize]).collect();
    let mut c = Canon { nparams, next: 0, inserted: 0 };
    let mut module = wasm_encoder::Module::new();
    c.parse_core_module(&mut module, Parser::new(0), bytes).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let _ = RoundtripReencoder; // keep the import used if the trait default changes
    Ok((module.finish(), c.inserted))
}
