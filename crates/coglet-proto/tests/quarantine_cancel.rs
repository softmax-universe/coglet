use coglet_proto::{quarantine::BundleSink, transfer::DurableSink};
use std::{
    fs,
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};

#[tokio::test]
async fn cancelled_append_discards_sink_even_if_caller_keeps_it() {
    let root = std::env::temp_dir().join(format!(
        "coglet-cancel-{}",
        iroh::SecretKey::generate().public()
    ));
    fs::create_dir(&root).unwrap();
    let mut sink = BundleSink::create(&root, 10).await.unwrap();
    {
        let mut append = pin!(sink.append(b"hello"));
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(append.as_mut().poll(&mut context), Poll::Pending));
    }
    assert!(
        sink.append(b"world").await.is_err(),
        "a cancelled write must permanently discard custody"
    );
    drop(sink);
    for _ in 0..100 {
        if fs::read_dir(&root).unwrap().count() == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    fs::remove_dir(&root).unwrap();
}
