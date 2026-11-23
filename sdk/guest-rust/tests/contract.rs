use coglet_guest_rust::{Context, Host, HostError, Operation, decode_events};

#[derive(Default)]
struct Recorder {
    calls: Vec<(Operation, Vec<u8>)>,
    result: i32,
}
impl Host for Recorder {
    fn call(&mut self, op: Operation, bytes: &[u8]) -> i32 {
        self.calls.push((op, bytes.to_vec()));
        self.result
    }
}

#[test]
fn stages_timer_with_unsigned_time_and_propagates_rejection() {
    let mut host = Recorder::default();
    let mut ctx = Context::new(&mut host, u64::MAX, u64::MAX, u64::MAX);
    let id = ctx.next_id().unwrap();
    assert_eq!(id.len(), 32);
    ctx.timer(&id, u64::MAX).unwrap();
    assert_eq!(host.calls.len(), 1);
    assert_eq!(host.calls[0].0, Operation::Timer);
    let v = coglet_types::decode_profile(&host.calls[0].1).unwrap();
    assert!(format!("{v:?}").contains("18446744073709551615"));
    host.result = 4;
    assert_eq!(
        Context::new(&mut host, 0, 0, 0).timer(&id, 1),
        Err(HostError::Conflict)
    );
}

#[test]
fn stages_emit_llm_and_log_in_order_and_validates_before_host() {
    use coglet_guest_rust::{Action, LlmRequest, LogEntry};
    let mut host = Recorder::default();
    let id = "a".repeat(32);
    let action = Action {
        id: id.clone(),
        coglet: format!("coglet:{}", "b".repeat(64)),
        capability: "test.echo".into(),
        args: Default::default(),
        deadline: u64::MAX,
        correlation_id: id.clone(),
    };
    let request = LlmRequest {
        id: id.clone(),
        correlation_id: id.clone(),
        capability: "llm.complete".into(),
        model: "mock".into(),
        input: Default::default(),
    };
    let entry = LogEntry {
        level: "info".into(),
        message: "count".into(),
        fields: Default::default(),
    };
    let mut ctx = Context::new(&mut host, 1, 2, 3);
    ctx.emit(&action).unwrap();
    ctx.llm(&request).unwrap();
    ctx.log(&entry).unwrap();
    assert_eq!(
        host.calls.iter().map(|c| c.0).collect::<Vec<_>>(),
        vec![Operation::Emit, Operation::Llm, Operation::Log]
    );
    let mut bad = request;
    bad.capability = "other.complete".into();
    assert!(Context::new(&mut host, 1, 2, 3).llm(&bad).is_err());
    assert_eq!(host.calls.len(), 3);
    for (code, error) in [
        (1, HostError::InvalidArgument),
        (2, HostError::PermissionDenied),
        (3, HostError::ResourceExhausted),
        (4, HostError::Conflict),
        (5, HostError::Reserved(5)),
    ] {
        host.result = code;
        assert_eq!(Context::new(&mut host, 0, 0, 0).log(&entry), Err(error));
    }
}

#[test]
fn filesystem_boundary_uses_host_overlay_not_native_root() {
    struct Overlay {
        bytes: Option<Vec<u8>>,
    }
    impl Host for Overlay {
        fn call(&mut self, _: Operation, _: &[u8]) -> i32 {
            0
        }
        fn read_file(&mut self, path: &str) -> Result<Option<Vec<u8>>, HostError> {
            assert_eq!(path, "/data/count");
            Ok(self.bytes.clone())
        }
        fn write_file(&mut self, path: &str, bytes: &[u8]) -> Result<(), HostError> {
            assert_eq!(path, "/data/count");
            self.bytes = Some(bytes.to_vec());
            Ok(())
        }
    }
    let mut host = Overlay { bytes: None };
    let mut ctx = Context::new(&mut host, 0, 0, 0);
    assert_eq!(ctx.read_file("/data/count").unwrap(), None);
    ctx.write_file("/data/count", b"1").unwrap();
    assert_eq!(ctx.read_file("/data/count").unwrap(), Some(b"1".to_vec()));
}

#[test]
fn rejects_empty_batch_and_noncanonical_encoding() {
    assert!(decode_events(&[0x80]).is_err());
    assert!(decode_events(&[0x98, 0]).is_err());
}
