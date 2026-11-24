use super::*;
use coglet_proto::wire::read_frame;
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture {
    _dir: tempfile::TempDir,
    hub: Endpoint,
    device: Endpoint,
    kernel: Kernel,
    hs: Session,
    dc: iroh::endpoint::Connection,
    hc: iroh::endpoint::Connection,
    serving: tokio::task::JoinHandle<()>,
    target: String,
    actor: String,
    ticket: String,
    time: Arc<AtomicU64>,
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let hub = Endpoint::builder(presets::Minimal)
            .alpns(vec![ALPN.to_vec()])
            .transport_config(
                iroh::endpoint::QuicTransportConfig::builder()
                    .send_window(1024)
                    .build(),
            )
            .bind()
            .await
            .unwrap();
        let device = Endpoint::builder(presets::Minimal)
            .transport_config(
                iroh::endpoint::QuicTransportConfig::builder()
                    .stream_receive_window(1024u32.into())
                    .max_concurrent_bidi_streams(16u32.into())
                    .build(),
            )
            .bind()
            .await
            .unwrap();
        let kernel = Kernel::open(dir.path(), hub.id(), Arc::new(|| Some(1))).unwrap();
        let actor = format!("hub:{}", hub.id());
        let ticket = "aa".repeat(16);
        let source = format!("surface:{}", SecretKey::generate().public());
        let mut r = registration(
            &SecretKey::generate(),
            hub.id(),
            device.id(),
            &source,
            &actor,
        );
        r.actions[0].ticket = Some(ticket.clone());
        kernel.open_window(Duration::from_secs(60)).await.unwrap();
        let invite = kernel
            .invite(
                device.id(),
                Duration::from_secs(60),
                vec![Grant {
                    method: "hub.actions".into(),
                    target: Some(r.coglet.clone()),
                    expires: 10000,
                    generation: 0,
                }],
            )
            .await
            .unwrap();
        let h = hub.clone();
        let accept = tokio::spawn(async move { h.accept().await.unwrap().await.unwrap() });
        let dc = device.connect(hub.addr(), ALPN).await.unwrap();
        let hc = accept.await.unwrap();
        let ds = Session::new(dc.clone(), device.id(), Role::Device).unwrap();
        let hs = Session::new(hc.clone(), hub.id(), Role::Hub).unwrap();
        let serving = tokio::spawn({
            let hs = hs.clone();
            let kernel = kernel.clone();
            async move {
                let _ = hs.serve(Arc::new(kernel)).await;
            }
        });
        assert_eq!(ds.request(json!({"method":"hub.enroll","target":null,"timeout_ms":1000,"correlation_id":"11".repeat(16),"params":{"offer":{"versions":["0.1"]},"invite":invite}})).await.unwrap()["type"],"reply");
        kernel.register(r.clone()).await.unwrap();

        Self {
            _dir: dir,
            hub,
            device,
            kernel,
            hs,
            dc,
            hc,
            serving,
            target: r.coglet,
            actor,
            ticket,
            time: Arc::new(AtomicU64::new(1)),
        }
    }
    async fn prepare(&self, large: bool) -> Value {
        let action = json!({"id":"44".repeat(16),"coglet":self.target,"capability":"test.move","args":{"text":if large {"x".repeat(200_000)} else {String::new()}},"deadline":200,"correlation_id":"22".repeat(16)});
        self.kernel
            .prepare_action(
                &self.target,
                action,
                json!({"actor":self.actor,"ticket":self.ticket}),
                Some(0),
            )
            .await
            .unwrap()
    }
    fn dispatch(&self) -> tokio::task::JoinHandle<coglet_hub::Result<Value>> {
        let k = self.kernel.clone();
        let t = self.target.clone();
        let s = self.hs.clone();
        let time = self.time.clone();
        tokio::spawn(async move {
            k.dispatch_action_with_clock(
                &t,
                &"44".repeat(16),
                s,
                Arc::new(move || {
                    let now = time.load(Ordering::SeqCst);
                    (now != u64::MAX).then_some(now)
                }),
            )
            .await
        })
    }
    async fn stop(&self, mode: &str, task: &tokio::task::JoinHandle<coglet_hub::Result<Value>>) {
        match mode {
            "abort" => task.abort(),
            "ticket" => self
                .kernel
                .end_ticket(&self.actor, &self.ticket)
                .await
                .unwrap(),
            "revoke" => self.kernel.revoke_peer(self.device.id()).await.unwrap(),
            "deadline" => {
                self.time.store(200, Ordering::SeqCst);
            }
            "clock_loss" => {
                self.time.store(u64::MAX, Ordering::SeqCst);
            }
            _ => panic!("unknown mode"),
        }
    }
    async fn close(self) {
        self.serving.abort();
        self.device.close().await;
        self.hub.close().await;
    }
}

#[tokio::test]
async fn blocked_first_write_resets_on_abort_ticket_revoke_deadline_and_clock_loss() {
    let _serial = SERIAL.acquire().await.unwrap();
    tokio::time::timeout(Duration::from_secs(30), async {
        for mode in ["abort","ticket","revoke","deadline","clock_loss"] {
            let f=Fixture::new().await;
            f.prepare(true).await;
            let task=f.dispatch();
            let (send,mut recv)=f.dc.accept_bi().await.unwrap();
            let mut prefix=[0u8;4]; recv.read_exact(&mut prefix).await.unwrap();
            assert!(u32::from_be_bytes(prefix)>100_000);
            // Do not consume body: 1KiB send/receive windows force write_all Pending.
            assert!(!task.is_finished(), "write was not blocked: {mode}");
            f.stop(mode,&task).await;
            assert_eq!(tokio::time::timeout(Duration::from_secs(1),send.stopped()).await.unwrap().unwrap().unwrap().into_inner(),1,"{mode}");
            assert!(recv.read_to_end(300_000).await.is_err(),"cancelled write finished instead of resetting: {mode}");
            // Deadline/clock-loss recovery queries a receipt, never a second action.
            if matches!(mode,"deadline"|"clock_loss") {
                let (mut s,mut r)=f.dc.accept_bi().await.unwrap();
                let request=read_frame(&mut r).await.unwrap();
                assert_eq!(request["method"],"hub.receipt");
                coglet_proto::wire::write_frame(&mut s,&json!({"type":"error","correlation_id":request["correlation_id"],"error":{"code":"not_found","message":"not admitted","retryable":false}})).await.unwrap();
                s.finish().unwrap();
            }
            if mode=="abort" { assert!(task.await.unwrap_err().is_cancelled()); }
            else {tokio::time::timeout(Duration::from_secs(1),task).await.unwrap().unwrap().unwrap();}
            f.close().await;
        }
    }).await.unwrap();
}

#[tokio::test]
async fn blocked_open_cannot_send_after_ticket_end_or_caller_abort() {
    let _serial = SERIAL.acquire().await.unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        for mode in ["ticket", "abort"] {
            let f = Fixture::new().await;
            f.prepare(false).await;
            let mut occupied = Vec::new();
            // Consume all actual QUIC bidi credit, outside Session's scheduler.
            for _ in 0..16 {
                let (mut send, recv) = f.hc.open_bi().await.unwrap();
                send.write_all(&[1]).await.unwrap();
                let (remote_send, mut remote_recv) = f.dc.accept_bi().await.unwrap();
                let mut b = [0];
                remote_recv.read_exact(&mut b).await.unwrap();
                occupied.push((send, recv, remote_send, remote_recv));
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(50), f.hc.open_bi())
                    .await
                    .is_err(),
                "fixture did not exhaust actual stream credit"
            );
            let task = f.dispatch();
            // Wait for durable attempt, then the per-poll driver to enter open_bi.
            let db = rusqlite::Connection::open(f._dir.path().join("kernel.db")).unwrap();
            loop {
                let attempted: i64 = db
                    .query_row("SELECT attempted FROM deliveries", [], |r| r.get(0))
                    .unwrap();
                if attempted == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
            f.stop(mode, &task).await;
            if mode == "abort" {
                assert!(task.await.unwrap_err().is_cancelled());
            } else {
                tokio::time::timeout(Duration::from_secs(1), task)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
            }
            drop(occupied);
            assert!(
                tokio::time::timeout(Duration::from_millis(100), f.dc.accept_bi())
                    .await
                    .is_err(),
                "cancelled open later delivered a request"
            );
            f.close().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn mapping_absence_overflow_and_restart_never_create_or_remap_intent() {
    let _serial = SERIAL.acquire().await.unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        let f=Fixture::new().await;
        let action=json!({"id":"44".repeat(16),"coglet":f.target,"capability":"test.move","args":{},"deadline":200,"correlation_id":"22".repeat(16)});
        let context=json!({"actor":f.actor,"ticket":f.ticket});
        assert!(f.kernel.prepare_action(&f.target,action.clone(),context.clone(),None).await.is_err());
        let mut overflow=action.clone();overflow["deadline"]=json!(u64::MAX);
        assert!(f.kernel.prepare_action(&f.target,overflow,context.clone(),Some(1)).await.is_err());
        let mut underflow=action.clone();underflow["deadline"]=json!(0);
        assert!(f.kernel.prepare_action(&f.target,underflow,context.clone(),Some(-1)).await.is_err());
        let db=rusqlite::Connection::open(f._dir.path().join("kernel.db")).unwrap();
        let count:i64=db.query_row("SELECT count(*) FROM deliveries",[],|r|r.get(0)).unwrap();assert_eq!(count,0);
        let mapped=f.kernel.prepare_action(&f.target,action.clone(),context.clone(),Some(1000)).await.unwrap();
        // A new Kernel process-equivalent reopens persisted Authority and deliveries;
        // no mapping or live runtime clock is reconstructed from stored offset/time.
        let reopened=Kernel::open(f._dir.path(),f.hub.id(),Arc::new(||Some(1))).unwrap();
        assert_eq!(reopened.prepare_action(&f.target,action,context,Some(-100)).await.unwrap(),mapped);
        assert!(reopened.dispatch_action(&f.target,&"44".repeat(16),f.hs.clone()).await.is_err());
        let attempted:i64=db.query_row("SELECT attempted FROM deliveries",[],|r|r.get(0)).unwrap();assert_eq!(attempted,0);
        drop(reopened);f.close().await;
    }).await.unwrap();
}

#[tokio::test]
async fn ticket_end_during_live_reply_wait_resets_and_survives_restart() {
    let _serial = SERIAL.acquire().await.unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        let f = Fixture::new().await;
        f.prepare(false).await;
        let task = f.dispatch();
        let (send, mut recv) = f.dc.accept_bi().await.unwrap();
        let request = read_frame(&mut recv).await.unwrap();
        assert_eq!(request["method"], "hub.actions");
        assert_eq!(recv.read(&mut [0]).await.unwrap(), None);
        f.kernel.end_ticket(&f.actor, &f.ticket).await.unwrap();
        assert_eq!(send.stopped().await.unwrap().unwrap().into_inner(), 1);
        let result = task.await.unwrap().unwrap();
        assert_eq!(result["error"]["details"]["ticket_ended"], true);
        let reopened = Kernel::open(f._dir.path(), f.hub.id(), Arc::new(|| Some(1))).unwrap();
        let db = rusqlite::Connection::open(f._dir.path().join("kernel.db")).unwrap();
        let saved: String = db
            .query_row("SELECT reply FROM deliveries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&saved).unwrap(), result);
        assert!(
            reopened
                .prepare_action(
                    &f.target,
                    request["params"]["action"].clone(),
                    request["context"].clone(),
                    None
                )
                .await
                .is_err()
        );
        drop(reopened);
        f.close().await;
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn deadline_admission_is_exclusive_and_lost_clock_never_marks_attempt() {
    let _serial = SERIAL.acquire().await.unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        let f = Fixture::new().await;
        let original = f.prepare(false).await;
        for now in [200, 201, u64::MAX] {
            f.time.store(now, Ordering::SeqCst);
            assert!(f.dispatch().await.unwrap().is_err());
            let db = rusqlite::Connection::open(f._dir.path().join("kernel.db")).unwrap();
            let row: (i64, String) = db
                .query_row("SELECT attempted,request FROM deliveries", [], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .unwrap();
            assert_eq!(row.0, 0);
            assert_eq!(serde_json::from_str::<Value>(&row.1).unwrap(), original);
        }
        f.close().await;
    })
    .await
    .unwrap();
}
