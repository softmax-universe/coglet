#[allow(dead_code)] // shared by generator and replay test with different entry points
#[path = "../tests/support/archive_corpus.rs"]
mod corpus;
fn main() {
    let (inventory, cases) = corpus::build();
    let dir = corpus::root().join("conformance/abi");
    std::fs::write(
        dir.join("executable-archive.fixture"),
        serde_json::to_vec_pretty(&inventory).unwrap(),
    )
    .unwrap();
    std::fs::write(
        dir.join("executable-archive.json"),
        serde_json::to_vec_pretty(&cases).unwrap(),
    )
    .unwrap();
    println!("Generated {} companion cases", cases.len());
}
