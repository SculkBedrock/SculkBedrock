use crate::server::Listener;
use parking_lot::RwLock;
use std::sync::Arc;
use sc_utils::game::structs::motd::Motd;

#[tokio::test]
#[ignore = "手动调试用：绑定 19132 端口无限 accept 永不退出，会挂死 cargo test；cargo test -- --ignored 手动运行"]
async fn test() {
    let listener = Listener::bind(
        "0.0.0.0:19132",
        Arc::new(RwLock::new(Motd::new(1234567890))),
    )
    .await
    .unwrap();
    listener.start().await.unwrap();
    loop {
        if let Ok(_) = listener.accept().await {
            println!("Connection accepted");
        }
    }
}
