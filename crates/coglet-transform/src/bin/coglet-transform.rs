//! Production pure rewrite executable. Binary stdin/stdout only: no trust,
//! signing keys, registry or implicit profile/feature widening.
use std::io::{Read, Write};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--source"] {
        std::io::stdout().write_all(coglet_transform::transform_source_bytes())?;
        return Ok(());
    }
    if !args.is_empty() {
        return Err("usage: coglet-transform [--source] <original.wasm >output.wasm".into());
    }
    let pins = coglet_transform::Pins::default();
    let mut original = Vec::new();
    std::io::stdin()
        .take(pins.max_bytes as u64 + 1)
        .read_to_end(&mut original)?;
    let rewritten = coglet_transform::rewrite(&original, &pins)?;
    std::io::stdout().write_all(&rewritten.output)?;
    Ok(())
}
