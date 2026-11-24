use super::*;

#[tokio::test]
async fn cancellation_audit_failure_rolls_back_ticket_reply_and_live_fence() {
    let dir = tempfile::tempdir().unwrap();
    let hub = iroh::SecretKey::generate().public();
    let k = Kernel::open(dir.path(), hub, Arc::new(|| Some(1))).unwrap();
    let actor = format!("hub:{hub}");
    let ticket = "aa".repeat(16);
    let target = format!("coglet:{}", iroh::SecretKey::generate().public());
    let id = "bb".repeat(16);
    {
        let s = k.state.lock().unwrap();
        s.store.db.execute("INSERT INTO deliveries(target,id,intent,request,attempted) VALUES(?1,?2,'{}',?3,1)",params![target,id,json!({"context":{"actor":actor,"ticket":ticket}}).to_string()]).unwrap();
        s.store.db.execute_batch("CREATE TRIGGER fail_cancel BEFORE INSERT ON audit BEGIN SELECT RAISE(ABORT,'audit unavailable'); END;").unwrap();
    }
    assert!(k.end_ticket(&actor, &ticket).await.is_err());
    {
        let s = k.state.lock().unwrap();
        let n: i64 = s
            .store
            .db
            .query_row("SELECT count(*) FROM ended_tickets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
        let reply: Option<String> = s
            .store
            .db
            .query_row("SELECT reply FROM deliveries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(reply, None);
        assert!(
            !k.boundary
                .lock()
                .unwrap()
                .ended
                .contains(&(actor.clone(), ticket.clone()))
        );
        s.store
            .db
            .execute_batch("DROP TRIGGER fail_cancel")
            .unwrap();
    }
    k.end_ticket(&actor, &ticket).await.unwrap();
    drop(k);
    let k = Kernel::open(dir.path(), hub, Arc::new(|| Some(1))).unwrap();
    let s = k.state.lock().unwrap();
    let n: i64 = s
        .store
        .db
        .query_row("SELECT count(*) FROM ended_tickets", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
    let reply: String = s
        .store
        .db
        .query_row("SELECT reply FROM deliveries", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&reply).unwrap()["error"]["details"]["ticket_ended"],
        true
    );
}

#[test]
fn queued_failed_cancelled_receipt_finalization_survives_restart() {
    for state in ["queued", "failed", "cancelled"] {
        let dir = tempfile::tempdir().unwrap();
        let hub = iroh::SecretKey::generate().public();
        let k = Kernel::open(dir.path(), hub, Arc::new(|| Some(1))).unwrap();
        let target = "target";
        let id = "id";
        let reply = json!({"type":"reply","correlation_id":id,"result":{"operation_id":id,"state":state,"step":null,"error":if state=="failed" {json!({"code":"unavailable","message":"receiver failed","retryable":false})} else {Value::Null}}});
        {
            let mut s = k.state.lock().unwrap();
            s.store.db.execute("INSERT INTO deliveries(target,id,intent,request,attempted) VALUES(?1,?2,'{}','{}',1)",params![target,id]).unwrap();
            assert_eq!(s.finish_action(target, id, reply.clone()).unwrap(), reply);
        }
        drop(k);
        let k = Kernel::open(dir.path(), hub, Arc::new(|| Some(1))).unwrap();
        let mut s = k.state.lock().unwrap();
        let saved: Option<String> = s
            .store
            .db
            .query_row("SELECT reply FROM deliveries", [], |r| r.get(0))
            .unwrap();
        if state == "queued" {
            assert_eq!(saved, None, "queued observation must remain reconcilable");
            let applied = json!({"type":"reply","correlation_id":id,"result":{"operation_id":id,"state":"applied","step":1,"error":null}});
            assert_eq!(
                s.finish_action(target, id, applied.clone()).unwrap(),
                applied
            );
        } else {
            assert_eq!(
                serde_json::from_str::<Value>(&saved.unwrap()).unwrap(),
                reply
            );
            assert_eq!(
                s.finish_action(target, id, json!({"result":{"state":"queued"}}))
                    .unwrap(),
                reply,
                "stale nonterminal receipt replaced terminal"
            );
        }
    }
}

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
