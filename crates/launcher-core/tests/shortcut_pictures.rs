mod support;

use std::time::Duration;

use launcher_core::net::meta::MetaClient;
use launcher_core::platform::shortcuts::{GRASS_BLOCK_PNG, build_picture};
use support::fake_files::{self, Served};

#[tokio::test]
async fn a_picture_at_an_address_is_fetched_for_the_shortcut() {
    let server = fake_files::start().await;
    server.put("icon.png", Served { body: GRASS_BLOCK_PNG.to_vec(), ..Served::default() });
    server.put("huge.png", Served { body: vec![0; 3 * 1024 * 1024], ..Served::default() });
    let meta = MetaClient::new(Duration::from_secs(5), Duration::from_secs(60)).unwrap();
    let fetched = build_picture(&meta, Some(&server.url("icon.png"))).await;
    assert_eq!(fetched.as_deref(), Some(GRASS_BLOCK_PNG));
    assert_eq!(build_picture(&meta, Some(&server.url("missing.png"))).await, None, "not found");
    assert_eq!(build_picture(&meta, Some(&server.url("huge.png"))).await, None, "too big for an icon");
    assert_eq!(build_picture(&meta, None).await, None);
}
