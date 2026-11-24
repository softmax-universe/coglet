use super::*;

#[tokio::test]
async fn grant_expiring_during_delegation_check_is_denied() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let dir = tempfile::tempdir().unwrap();
    let hub = iroh::SecretKey::generate().public();
    let peer = iroh::SecretKey::generate().public();
    let key = iroh::SecretKey::generate();
    let target = format!("coglet:{}", key.public());
    let armed = Arc::new(AtomicBool::new(false));
    let samples = Arc::new(AtomicUsize::new(0));
    let clock = {
        let armed = armed.clone();
        let samples = samples.clone();
        Arc::new(move || {
            Some(
                if armed.load(Ordering::SeqCst) && samples.fetch_add(1, Ordering::SeqCst) > 0 {
                    100
                } else {
                    1
                },
            )
        })
    };
    let kernel = Kernel::open(dir.path(), hub, clock).unwrap();
    kernel.open_window(Duration::from_secs(60)).await.unwrap();
    let invite = kernel
        .invite(
            peer,
            Duration::from_secs(60),
            vec![Grant {
                method: "hub.events".into(),
                target: Some(target.clone()),
                expires: 100,
                generation: 0,
            }],
        )
        .await
        .unwrap();
    kernel
        .state
        .lock()
        .unwrap()
        .authority
        .enroll(peer, Some(&invite))
        .unwrap();
    let statement = json!({"purpose":"delegation","hub":hub.to_string(),"device":peer.to_string(),"coglet":target,"generation":1,"expires":1000,"methods":["hub.events"]});
    let signature = key.sign(&coglet_proto::signing::preimage("delegation", &statement).unwrap());
    kernel.register(Registration {coglet:target.clone(),device:peer.to_string(),grant_generation:0,delegation:json!({"statement":statement,"signer":key.public().to_string(),"signature":hex::encode(signature.to_bytes())}),events:vec![],actions:vec![]}).await.unwrap();
    armed.store(true, Ordering::SeqCst);
    assert!(
        kernel
            .state
            .lock()
            .unwrap()
            .authorize(peer, &target, "hub.events")
            .is_err(),
        "grant was checked only against pre-delegation time"
    );
}
#[tokio::test]
async fn disconnect_is_not_lost_when_request_admission_is_saturated() {
    let dir = tempfile::tempdir().unwrap();
    let hub = iroh::SecretKey::generate().public();
    let peer = iroh::SecretKey::generate().public();
    let k = Kernel::open(dir.path(), hub, Arc::new(|| Some(1))).unwrap();
    let token = k.transport_connected(peer).await.unwrap();
    let permits = k.blocking.acquire_many(8).await.unwrap();
    k.transport_disconnected(peer, token).await.unwrap();
    drop(permits);
    assert_eq!(
        k.status().await.unwrap()["devices"][0]["state"],
        "disconnected"
    );
}
