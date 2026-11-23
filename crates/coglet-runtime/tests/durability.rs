use coglet_runtime::DurableStore;
use serde_json::json;
use std::{
    fs::{self, OpenOptions},
    io::Write,
};

#[test]
fn durable_commit_recovers_last_complete_transaction_and_discards_partial_tail() {
    let directory = std::env::temp_dir().join(format!("coglet-wal-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("runtime.wal");
    let first = json!({"files":{},"queue":[1],"outbox":[]});
    let second = json!({"files":{},"queue":[],"outbox":[1]});
    {
        let mut wal = DurableStore::open(&path).unwrap();
        wal.commit(&first, None).unwrap();
        wal.commit(&second, None).unwrap();
    }
    let good_length = fs::metadata(&path).unwrap().len();
    {
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&[0, 0, 0, 10, 0xa1]).unwrap();
        file.sync_all().unwrap();
    }
    let mut recovered = DurableStore::open(&path).unwrap();
    assert_eq!(recovered.state(), Some(&second));
    assert_eq!(fs::metadata(&path).unwrap().len(), good_length);
    recovered.commit(&first, None).unwrap();
    drop(recovered);
    assert_eq!(DurableStore::open(&path).unwrap().state(), Some(&first));
    fs::remove_dir_all(directory).unwrap();
}
