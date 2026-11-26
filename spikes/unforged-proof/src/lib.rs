#![deny(warnings)]
//! Coglet x unforged proof spike. Opt-in; never part of the coglet workspace.
//!
//! `counter` is the shared step function (real `coglets/counter` under coglet's
//! `Runtime::step` + WAMR); `unf` wraps the unforged APIs the acts share.
pub mod act0;
pub mod act1;
pub mod act2;
pub mod act3;
pub mod act4;
pub mod counter;
pub mod report;
pub mod unf;
