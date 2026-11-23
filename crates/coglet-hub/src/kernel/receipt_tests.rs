use super::*;

#[tokio::test]
async fn queued_receipt_cannot_mask_committed_ticket_end() {
    let dir = tempfile::tempdir().unwrap();
    let hub = iroh::SecretKey::generate().public();
    let kernel = Kernel::open(dir.path(), hub, Arc::new(|| Some(1))).unwrap();
    let actor = format!("hub:{hub}");
    let ticket = "aa".repeat(16);
    let target = format!("coglet:{}", iroh::SecretKey::generate().public());
    let id = "bb".repeat(16);
    let request = json!({"context":{"actor":actor,"ticket":ticket}});
    {
        let s = kernel.state.lock().unwrap();
        s.store.db.execute(
            "INSERT INTO deliveries(target,id,intent,request,attempted) VALUES(?1,?2,'{}',?3,1)",
            params![target,id,request.to_string()],
        ).unwrap();
    }
    // A remote receipt has arrived; before its local finalization the operator
    // commits ticket cancellation. No sleeps or transport timing assumptions.
    let queued =
        json!({"type":"reply","correlation_id":id,"result":{"operation_id":id,"state":"queued"}});
    // With no terminal state a queued observation remains nonterminal and
    // can be reconciled again; it must not consume the eventual cancellation.
    {
        let mut s = kernel.state.lock().unwrap();
        assert_eq!(
            s.finish_action(&target, &id, queued.clone()).unwrap(),
            queued
        );
        let reply: Option<String> = s
            .store
            .db
            .query_row("SELECT reply FROM deliveries", [], |r| r.get(0))
            .unwrap();
        assert!(reply.is_none());
    }
    kernel.end_ticket(&actor, &ticket).await.unwrap();
    let terminal: Value = {
        let s = kernel.state.lock().unwrap();
        let text: String = s
            .store
            .db
            .query_row("SELECT reply FROM deliveries", [], |r| r.get(0))
            .unwrap();
        serde_json::from_str(&text).unwrap()
    };
    assert_eq!(terminal["error"]["details"]["ticket_ended"], true);
    let returned = kernel
        .state
        .lock()
        .unwrap()
        .finish_action(&target, &id, queued)
        .unwrap();
    assert_eq!(
        returned, terminal,
        "a stale queued receipt must return the committed terminal result"
    );
    drop(kernel);
    let reopened = Kernel::open(dir.path(), hub, Arc::new(|| Some(1))).unwrap();
    let text: String = reopened
        .state
        .lock()
        .unwrap()
        .store
        .db
        .query_row("SELECT reply FROM deliveries", [], |r| r.get(0))
        .unwrap();
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), terminal);
}
