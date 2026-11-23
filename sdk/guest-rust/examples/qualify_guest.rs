//! Apply the existing R3 transform; engine admission independently checks output.
#[rustfmt::skip]
#[path = "../../../spikes/cross-engine-hash/host-wasmtime/src/nancanon.rs"]
mod nancanon;
fn main() -> anyhow::Result<()> {
    for path in std::env::args().skip(1) {
        let bytes = std::fs::read(&path)?;
        let (qualified, count) = nancanon::canonicalize(&bytes)?;
        wasmparser::Validator::new().validate_all(&qualified)?;
        std::fs::write(&path, qualified)?;
        eprintln!("R3 {path}: canonicalized {count} operators");
    }
    Ok(())
}
