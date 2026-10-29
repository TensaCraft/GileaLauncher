//! The module against a fake Minecraft Services and Mojang.

mod support;

use module_skins::backend::mojang::{CallError, Endpoints, MojangClient};
use module_skins::dto::SkinVariant;
use support::fake_services::{self, Fake, Player, TOKEN};

fn client(fake: &Fake) -> MojangClient {
    MojangClient::new(Endpoints::local(&fake.base)).unwrap()
}

fn wear(fake: &Fake, texture: &str, slim: bool) {
    fake.account().skin = Some((texture.into(), slim));
}

fn two_capes(fake: &Fake) {
    let mut account = fake.account();
    account.capes = vec![
        ("c1".into(), "Migrator".into(), "cape1".into()),
        ("c2".into(), "Vanilla".into(), "cape2".into()),
    ];
    account.active_cape = Some("c1".into());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_profile_is_read() {
    let fake = fake_services::start().await;
    wear(&fake, "steve", true);
    two_capes(&fake);
    let profile = client(&fake).profile(TOKEN).await.unwrap();
    assert_eq!(profile.skins.len(), 1);
    assert_eq!(profile.skins[0].state, "ACTIVE");
    assert_eq!(profile.skins[0].variant, SkinVariant::Slim, "Mojang's SLIM reads as slim");
    assert_eq!(profile.skins[0].url, fake.texture_url("steve"));
    let capes: Vec<(&str, &str, &str)> =
        profile.capes.iter().map(|c| (c.id.as_str(), c.alias.as_str(), c.state.as_str())).collect();
    assert_eq!(capes, [("c1", "Migrator", "ACTIVE"), ("c2", "Vanilla", "INACTIVE")]);
    assert_eq!(fake.requests("/profile")[0].bearer.as_deref(), Some(TOKEN));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_skin_is_uploaded_as_multipart() {
    let fake = fake_services::start().await;
    let png = b"\x89PNG fake skin".to_vec();
    let profile = client(&fake).upload_skin(TOKEN, png.clone(), SkinVariant::Slim).await.unwrap();
    let sent = &fake.requests("/skins")[0];
    assert_eq!((sent.method.as_str(), sent.bearer.as_deref()), ("POST", Some(TOKEN)));
    assert_eq!(sent.fields.get("variant").map(Vec::as_slice), Some(&b"slim"[..]));
    assert_eq!(sent.fields.get("file"), Some(&png));
    assert_eq!(profile.skins[0].variant, SkinVariant::Slim, "the answer is the new profile");
    assert!(fake.requests("/profile").is_empty(), "no second request when the answer is a profile");

    fake.knobs().bare_answers = true;
    let profile = client(&fake).upload_skin(TOKEN, png, SkinVariant::Classic).await.unwrap();
    assert_eq!(profile.skins[0].variant, SkinVariant::Classic);
    assert_eq!(fake.requests("/profile").len(), 1, "an empty answer is followed by the profile");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cape_is_put_on_and_taken_off() {
    let fake = fake_services::start().await;
    two_capes(&fake);
    let client = client(&fake);
    let profile = client.set_cape(TOKEN, Some("c2")).await.unwrap();
    let put = &fake.requests("/capes")[0];
    assert_eq!(put.method, "PUT");
    assert_eq!(put.json, Some(serde_json::json!({"capeId": "c2"})));
    let active: Vec<&str> =
        profile.capes.iter().filter(|c| c.state == "ACTIVE").map(|c| c.id.as_str()).collect();
    assert_eq!(active, ["c2"]);
    let profile = client.set_cape(TOKEN, None).await.unwrap();
    assert_eq!(fake.requests("/capes")[1].method, "DELETE");
    assert!(profile.capes.iter().all(|c| c.state != "ACTIVE"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_player_s_skin_is_found_by_name() {
    let fake = fake_services::start().await;
    fake.add_player(Player {
        id: "aa11".into(),
        name: "Steve_42".into(),
        texture: Some("s42".into()),
        slim: false,
    });
    fake.add_player(Player {
        id: "bb22".into(),
        name: "Alex".into(),
        texture: Some("alex".into()),
        slim: true,
    });
    let client = client(&fake);
    assert_eq!(
        client.player_skin("  Steve_42 ").await.unwrap(),
        (fake.texture_url("s42"), SkinVariant::Classic)
    );
    assert_eq!(client.player_skin("Alex").await.unwrap(), (fake.texture_url("alex"), SkinVariant::Slim));
    assert_eq!(fake.requests("/name/Steve_42").len(), 1, "the name is trimmed");
    assert_eq!(fake.requests("/session/aa11").len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_player_is_not_found() {
    let fake = fake_services::start().await;
    let client = client(&fake);
    assert_eq!(client.player_skin("Nobody").await, Err(CallError::NotFound));
    fake.knobs().unknown_is_204 = true;
    assert_eq!(client.player_skin("Nobody").await, Err(CallError::NotFound));
    let refused = client.player_skin("two words").await;
    assert!(matches!(refused, Err(CallError::Other(_))), "{refused:?}");
    assert!(fake.all_requests().iter().all(|r| !r.path.contains("two")), "a bad name asks nothing");
}

#[tokio::test(flavor = "multi_thread")]
async fn statuses_map_to_errors() {
    let fake = fake_services::start().await;
    let client = client(&fake);
    fake.knobs().valid_token = Some("another".into());
    assert_eq!(client.profile(TOKEN).await, Err(CallError::Unauthorized));
    fake.knobs().valid_token = None;
    fake.knobs().profile_status = Some(429);
    assert_eq!(client.profile(TOKEN).await, Err(CallError::Rate));
    fake.knobs().upload_status = Some(400);
    assert_eq!(client.upload_skin(TOKEN, vec![1], SkinVariant::Classic).await, Err(CallError::Rejected));
    fake.knobs().cape_status = Some(500);
    assert!(matches!(client.set_cape(TOKEN, None).await, Err(CallError::Other(_))));
}
