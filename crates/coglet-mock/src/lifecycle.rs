//! Real-i roh lifecycle peer adapter. The supplied durable runtime handler owns
//! all receipts and effects. This adapter never manufactures a successful body.
use coglet_proto::transport::{ALPN, OperationHandler, Role, Session};
use iroh::{Endpoint, SecretKey, endpoint::presets};
use std::sync::Arc;
use tokio::task::JoinHandle;

pub struct LifecyclePeer {
    endpoint: Endpoint,
    task: JoinHandle<()>,
}
impl LifecyclePeer {
    /// Construct the handler after binding so its target pins use the actual
    /// endpoint identity. A fresh Session may be made after handler restart.
    pub async fn bind(key: SecretKey) -> std::io::Result<Endpoint> {
        Endpoint::builder(presets::Minimal)
            .secret_key(key)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await
            .map_err(std::io::Error::other)
    }
    pub async fn connect(
        endpoint: Endpoint,
        router: &Endpoint,
        handler: Arc<dyn OperationHandler>,
    ) -> std::io::Result<(Self, Session, Session)> {
        let outgoing = endpoint.connect(router.addr(), ALPN);
        let incoming = async {
            router
                .accept()
                .await
                .ok_or_else(|| std::io::Error::other("router closed"))?
                .await
                .map_err(std::io::Error::other)
        };
        let (device, hub) = tokio::try_join!(
            async { outgoing.await.map_err(std::io::Error::other) },
            incoming
        )?;
        let device =
            Session::new(device, endpoint.id(), Role::Device).map_err(std::io::Error::other)?;
        let hub = Session::new(hub, router.id(), Role::Hub).map_err(std::io::Error::other)?;
        let serving = device.clone();
        let task = tokio::spawn(async move {
            let _ = serving.serve(handler).await;
        });
        Ok((Self { endpoint, task }, device, hub))
    }
    pub async fn close(self) {
        self.task.abort();
        let _ = self.task.await;
        self.endpoint.close().await;
    }
}
