#![cfg(feature = "database")]
use coglet_guest_rust::database::{Client, DbError, DbHost};
use coglet_types::{DatabaseDbPage, DatabaseDbStatement, Value};

struct Transport {
    result: i32,
    raw: Vec<u8>,
    calls: usize,
}
impl DbHost for Transport {
    fn db(&mut self, request: &[u8], output: &mut [u8]) -> i32 {
        self.calls += 1;
        assert_eq!(output.len(), 65_540);
        coglet_types::validate(
            "database::db-request",
            &coglet_types::decode_profile(request).unwrap(),
        )
        .unwrap();
        output[..4].copy_from_slice(&(self.raw.len() as u32).to_be_bytes());
        let n = self.raw.len().min(output.len() - 4);
        output[4..4 + n].copy_from_slice(&self.raw[..n]);
        self.result
    }
}
fn request() -> DatabaseDbStatement {
    DatabaseDbStatement {
        operation: "statement".into(),
        statement: "objects_get".into(),
        mode: "read".into(),
        params: vec![],
        max_rows: 64,
        max_bytes: 65536,
        cursor: None,
    }
}
fn page() -> DatabaseDbPage {
    DatabaseDbPage {
        status: "page".into(),
        columns: vec!["id".into()],
        rows: vec![vec![Value::Text("a".into())]],
        affected: 0,
        revision: u64::MAX,
        next_cursor: Value::Null,
    }
}
#[test]
fn db_buffer_reply_roundtrips_without_narrowing_revision() {
    let mut host = Transport {
        result: 0,
        raw: page().to_cbor().unwrap(),
        calls: 0,
    };
    let actual = Client::new(&mut host).statement(&request()).unwrap();
    assert_eq!(actual.revision, u64::MAX);
    assert_eq!(actual.rows, page().rows);
    assert_eq!(host.calls, 1);
}
#[test]
fn nonzero_result_never_decodes_output() {
    let mut host = Transport {
        result: 4,
        raw: vec![0xff],
        calls: 0,
    };
    assert!(matches!(
        Client::new(&mut host).statement(&request()),
        Err(DbError::Host(coglet_guest_rust::HostError::Conflict))
    ));
}
#[test]
fn reply_length_prefix_and_semantic_bounds_fail_closed() {
    let mut variants = vec![vec![], vec![0; 65537]];
    let mut malformed = page();
    malformed.rows = vec![vec![]];
    variants.push(malformed.to_cbor().unwrap());
    malformed = page();
    malformed.rows[0][0] = Value::Text("é".repeat(2049));
    variants.push(malformed.to_cbor().unwrap());
    malformed = page();
    malformed.next_cursor = Value::Text("é".repeat(257));
    variants.push(malformed.to_cbor().unwrap());
    for raw in variants {
        let mut host = Transport {
            result: 0,
            raw,
            calls: 0,
        };
        assert!(Client::new(&mut host).statement(&request()).is_err());
    }
}
#[test]
fn enqueue_binds_first_insert_id_but_accepts_original_duplicate_id() {
    use coglet_types::{DatabaseDbEnqueue, DatabaseDbEnqueued};
    let req = DatabaseDbEnqueue {
        operation: "enqueue".into(),
        intent_id: "1".repeat(32),
        idempotency_key: "2".repeat(32),
        kind: "test.run".into(),
        payload: Default::default(),
        correlation_id: "3".repeat(32),
    };
    for duplicate in [false, true] {
        let reply = DatabaseDbEnqueued {
            status: "enqueued".into(),
            intent_id: "4".repeat(32),
            duplicate,
        };
        let mut host = Transport {
            result: 0,
            raw: reply.to_cbor().unwrap(),
            calls: 0,
        };
        assert_eq!(Client::new(&mut host).enqueue(&req).is_ok(), duplicate);
    }
}
#[test]
fn invalid_request_is_refused_before_import() {
    let mut host = Transport {
        result: 0,
        raw: page().to_cbor().unwrap(),
        calls: 0,
    };
    let mut req = request();
    req.params.push(Value::Text("é".repeat(2049)));
    assert!(Client::new(&mut host).statement(&req).is_err());
    assert_eq!(host.calls, 0);
}
