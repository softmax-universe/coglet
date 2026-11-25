//! Independent R3 oracle. Not an engine and not a second copy of the rewriter.
use crate::{F32_CANON, F64_CANON};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Width {
    F32,
    F64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Sqrt,
    Ceil,
    Floor,
    Trunc,
    Nearest,
    Min,
    Max,
    Promote,
    Demote,
    Neg,
    Abs,
    Copysign,
}

impl Op {
    pub fn rewritten(self) -> bool {
        !matches!(self, Self::Neg | Self::Abs | Self::Copysign)
    }
    pub fn all_r3() -> &'static [Self] {
        &[
            Self::Add,
            Self::Sub,
            Self::Mul,
            Self::Div,
            Self::Sqrt,
            Self::Ceil,
            Self::Floor,
            Self::Trunc,
            Self::Nearest,
            Self::Min,
            Self::Max,
            Self::Promote,
            Self::Demote,
        ]
    }
    pub fn wat(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Sub => "sub",
            Self::Mul => "mul",
            Self::Div => "div",
            Self::Sqrt => "sqrt",
            Self::Ceil => "ceil",
            Self::Floor => "floor",
            Self::Trunc => "trunc",
            Self::Nearest => "nearest",
            Self::Min => "min",
            Self::Max => "max",
            Self::Promote => "promote_f32",
            Self::Demote => "demote_f64",
            Self::Neg => "neg",
            Self::Abs => "abs",
            Self::Copysign => "copysign",
        }
    }
    pub fn arity(self) -> u8 {
        match self {
            Self::Add
            | Self::Sub
            | Self::Mul
            | Self::Div
            | Self::Min
            | Self::Max
            | Self::Copysign => 2,
            _ => 1,
        }
    }
}

pub fn r3_bits(op: Op, width: Width, a: u64, b: u64) -> u64 {
    match (op, width) {
        (Op::Promote, Width::F64) => canon64(f64::from(f32::from_bits(a as u32))),
        (Op::Demote, Width::F32) => u64::from(canon32(f64::from_bits(a) as f32)),
        (Op::Promote, Width::F32) | (Op::Demote, Width::F64) => 0,
        (_, Width::F32) => u64::from(r3_f32(op, a as u32, b as u32)),
        (_, Width::F64) => r3_f64(op, a, b),
    }
}

fn canon32(x: f32) -> u32 {
    if x.is_nan() { F32_CANON } else { x.to_bits() }
}
fn canon64(x: f64) -> u64 {
    if x.is_nan() { F64_CANON } else { x.to_bits() }
}

fn wasm_min32(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        return f32::from_bits(F32_CANON);
    }
    if a == 0.0 && b == 0.0 {
        return if a.is_sign_negative() || b.is_sign_negative() {
            -0.0
        } else {
            0.0
        };
    }
    if a < b { a } else { b }
}
fn wasm_max32(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        return f32::from_bits(F32_CANON);
    }
    if a == 0.0 && b == 0.0 {
        return if a.is_sign_negative() && b.is_sign_negative() {
            -0.0
        } else {
            0.0
        };
    }
    if a > b { a } else { b }
}
fn wasm_min64(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        return f64::from_bits(F64_CANON);
    }
    if a == 0.0 && b == 0.0 {
        return if a.is_sign_negative() || b.is_sign_negative() {
            -0.0
        } else {
            0.0
        };
    }
    if a < b { a } else { b }
}
fn wasm_max64(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        return f64::from_bits(F64_CANON);
    }
    if a == 0.0 && b == 0.0 {
        return if a.is_sign_negative() && b.is_sign_negative() {
            -0.0
        } else {
            0.0
        };
    }
    if a > b { a } else { b }
}

fn nearest32(x: f32) -> f32 {
    let y = x.round_ties_even();
    if y == 0.0 { 0.0_f32.copysign(x) } else { y }
}
fn nearest64(x: f64) -> f64 {
    let y = x.round_ties_even();
    if y == 0.0 { 0.0_f64.copysign(x) } else { y }
}

fn r3_f32(op: Op, a: u32, b: u32) -> u32 {
    let x = f32::from_bits(a);
    let y = f32::from_bits(b);
    match op {
        Op::Add => canon32(x + y),
        Op::Sub => canon32(x - y),
        Op::Mul => canon32(x * y),
        Op::Div => canon32(x / y),
        Op::Sqrt => canon32(x.sqrt()),
        Op::Ceil => canon32(x.ceil()),
        Op::Floor => canon32(x.floor()),
        Op::Trunc => canon32(x.trunc()),
        Op::Nearest => canon32(nearest32(x)),
        Op::Min => canon32(wasm_min32(x, y)),
        Op::Max => canon32(wasm_max32(x, y)),
        Op::Neg => (-x).to_bits(),
        Op::Abs => x.abs().to_bits(),
        Op::Copysign => x.copysign(y).to_bits(),
        Op::Promote | Op::Demote => 0,
    }
}

fn r3_f64(op: Op, a: u64, b: u64) -> u64 {
    let x = f64::from_bits(a);
    let y = f64::from_bits(b);
    match op {
        Op::Add => canon64(x + y),
        Op::Sub => canon64(x - y),
        Op::Mul => canon64(x * y),
        Op::Div => canon64(x / y),
        Op::Sqrt => canon64(x.sqrt()),
        Op::Ceil => canon64(x.ceil()),
        Op::Floor => canon64(x.floor()),
        Op::Trunc => canon64(x.trunc()),
        Op::Nearest => canon64(nearest64(x)),
        Op::Min => canon64(wasm_min64(x, y)),
        Op::Max => canon64(wasm_max64(x, y)),
        Op::Neg => (-x).to_bits(),
        Op::Abs => x.abs().to_bits(),
        Op::Copysign => x.copysign(y).to_bits(),
        Op::Promote | Op::Demote => 0,
    }
}
