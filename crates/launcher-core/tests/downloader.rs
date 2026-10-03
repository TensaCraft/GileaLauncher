mod support;

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use launcher_core::net::downloader::{
    Credential, DownloadProgress, DownloadReport, DownloadTask, Downloader, DownloaderConfig, ExpectedHash,
    Origin,
};
use launcher_shared::{AppResult, ErrorCode};
use sha1::{Digest, Sha1};
use support::fake_files::{self, FileServer, Served};

fn fast() -> DownloaderConfig {
    DownloaderConfig {
        retry_delay: Duration::from_millis(1),
        timeout: Duration::from_secs(5),
        ..DownloaderConfig::default()
    }
}

/// Files up to 32 KiB come whole; the larger ones here stream, and resume.
fn downloader() -> Downloader {
    Downloader::new(DownloaderConfig { in_memory_up_to: 32 * 1024, ..fast() }).unwrap()
}

fn sha1_hex(bytes: &[u8]) -> String {
    hex::encode(Sha1::digest(bytes))
}

fn content(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect()
}

fn task(server: &FileServer, name: &str, dir: &Path, expected: &[u8]) -> DownloadTask {
    DownloadTask::new(server.url(name), dir.join(name))
        .size(expected.len() as u64)
        .hash(ExpectedHash::sha1(&sha1_hex(expected)))
}

async fn run(tasks: Vec<DownloadTask>) -> AppResult<DownloadReport> {
    downloader().download_all(tasks, false, &|_| {}).await
}

fn leftovers(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".part"))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn downloads_and_verifies_files() {
    let server = fake_files::start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut tasks = Vec::new();
    for i in 0..3 {
        let body = content(40_000 + i, i as u8);
        server.put(&format!("f{i}.jar"), Served { body: body.clone(), ..Served::default() });
        tasks.push(task(&server, &format!("f{i}.jar"), dir.path(), &body));
    }
    let report = run(tasks).await.unwrap();
    assert_eq!((report.downloaded, report.skipped, report.failed.len()), (3, 0, 0));
    assert_eq!(std::fs::read(dir.path().join("f1.jar")).unwrap(), content(40_001, 1));
    assert!(leftovers(dir.path()).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn present_files_are_skipped_and_damaged_ones_replaced() {
    let server = fake_files::start().await;
    let dir = tempfile::tempdir().unwrap();
    let body = content(10_000, 7);
    server.put("a.jar", Served { body: body.clone(), ..Served::default() });
    run(vec![task(&server, "a.jar", dir.path(), &body)]).await.unwrap();
    let again = run(vec![task(&server, "a.jar", dir.path(), &body)]).await.unwrap();
    assert_eq!((again.downloaded, again.skipped), (0, 1));
    assert_eq!(server.total_requests(), 1);
    std::fs::write(dir.path().join("a.jar"), vec![0u8; body.len()]).unwrap();
    let verified = downloader()
        .download_all(vec![task(&server, "a.jar", dir.path(), &body)], true, &|_| {})
        .await
        .unwrap();
    assert_eq!((verified.downloaded, verified.skipped), (1, 0));
    assert_eq!(std::fs::read(dir.path().join("a.jar")).unwrap(), body);
}

#[tokio::test(flavor = "multi_thread")]
async fn hash_mismatch_is_a_failure_without_a_file() {
    let server = fake_files::start().await;
    let dir = tempfile::tempdir().unwrap();
    let body = content(5_000, 1);
    server.put("bad.jar", Served { body: content(5_000, 2), ..Served::default() });
    let report = run(vec![task(&server, "bad.jar", dir.path(), &body)]).await.unwrap();
    assert_eq!(report.failed.len(), 1);
    assert!(report.failed[0].starts_with("bad.jar: sha1 mismatch"), "{:?}", report.failed);
    assert!(!dir.path().join("bad.jar").exists());
    assert!(leftovers(dir.path()).is_empty());
    let err = report.into_result().unwrap_err();
    assert_eq!(err.code, ErrorCode::DownloadFailed);
    assert!(err.params["error"].starts_with("bad.jar"));
}

#[tokio::test(flavor = "multi_thread")]
async fn server_errors_are_retried() {
    let server = fake_files::start().await;
    let dir = tempfile::tempdir().unwrap();
    let body = content(3_000, 3);
    server.put("flaky.jar", Served { body: body.clone(), fail_times: 2, ..Served::default() });
    let report = run(vec![task(&server, "flaky.jar", dir.path(), &body)]).await.unwrap();
    assert_eq!(report.downloaded, 1);
    assert_eq!(server.seen("flaky.jar").len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_asking_to_wait_is_waited_for() {
    let server = fake_files::start().await;
    let dir = tempfile::tempdir().unwrap();
    let body = content(3_000, 4);
    server.put(
        "busy.jar",
        Served { body: body.clone(), fail_times: 1, throttle: Some(1), ..Served::default() },
    );
    let started = std::time::Instant::now();
    let report = run(vec![task(&server, "busy.jar", dir.path(), &body)]).await.unwrap();
    assert_eq!(report.downloaded, 1);
    assert!(started.elapsed() >= Duration::from_millis(900), "Retry-After is kept: {:?}", started.elapsed());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_network_that_is_down_stops_the_batch_soon() {
    // Nothing answers on port 1: every connection is refused, as with the network down.
    let dir = tempfile::tempdir().unwrap();
    let tasks: Vec<DownloadTask> = (0..40)
        .map(|i| {
            DownloadTask::new(
                format!("http://127.0.0.1:1/file-{i}.jar"),
                dir.path().join(format!("f{i}.jar")),
            )
        })
        .collect();
    let report = run(tasks).await.unwrap();
    assert_eq!(report.failed.len(), 40);
    let gave_up = report.failed.iter().filter(|f| f.contains("the network is unreachable")).count();
    assert!(gave_up > 0, "the files left are not tried once the network is down: {:?}", report.failed);
    let error = report.into_result().unwrap_err();
    assert_eq!(error.code, ErrorCode::Network, "{error:?}");
    assert!(error.detail.len() < 2_000, "one reason, not forty: {}", error.detail.len());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_api_client_kept_to_its_hosts_follows_no_redirect_elsewhere() {
    // A client that carries a key (CurseForge's) never takes it to another host.
    let server = fake_files::start().await;
    let elsewhere = server.url("target").replace("127.0.0.1", "localhost");
    server.put("moved", Served { redirect_to: Some(elsewhere), ..Served::default() });
    server.put("target", Served { body: b"secret".to_vec(), ..Served::default() });
    let within = std::sync::Arc::new(|url: &reqwest::Url| url.host_str() == Some("127.0.0.1"));
    let client =
        launcher_core::net::api_client_within("test", Duration::from_secs(5), Duration::from_secs(5), within)
            .unwrap();
    let error = client.get(server.url("moved")).send().await.unwrap_err();
    assert!(error.is_redirect(), "{error:?}");
    assert!(server.seen("target").is_empty(), "the other host is never asked");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_api_asks_a_busy_server_again() {
    let server = fake_files::start().await;
    server
        .put("busy", Served { body: b"{}".to_vec(), fail_times: 1, throttle: Some(1), ..Served::default() });
    server.put("down", Served { body: b"{}".to_vec(), fail_times: 2, ..Served::default() });
    let client =
        launcher_core::net::api_client("test", Duration::from_secs(5), Duration::from_secs(5)).unwrap();
    let started = std::time::Instant::now();
    let busy = launcher_core::net::send_patiently(client.get(server.url("busy"))).await.unwrap();
    assert_eq!(busy.status(), 200);
    assert!(started.elapsed() >= Duration::from_millis(900), "Retry-After is kept");
    let down = launcher_core::net::send_patiently(client.get(server.url("down"))).await.unwrap();
    assert_eq!((down.status(), server.seen("down").len()), (reqwest::StatusCode::OK, 3));
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_files_fail_without_retries() {
    let server = fake_files::start().await;
    let dir = tempfile::tempdir().unwrap();
    server.put("gone.jar", Served { status: Some(404), ..Served::default() });
    let report =
        run(vec![DownloadTask::new(server.url("gone.jar"), dir.path().join("gone.jar"))]).await.unwrap();
    assert!(report.failed[0].contains("404"), "{:?}", report.failed);
    assert_eq!(server.seen("gone.jar").len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn only_https_is_allowed() {
    let dir = tempfile::tempdir().unwrap();
    let report =
        run(vec![DownloadTask::new("http://example.com/a.jar", dir.path().join("a.jar"))]).await.unwrap();
    assert!(report.failed[0].contains("https"), "{:?}", report.failed);
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_answers_are_cut_off() {
    let server = fake_files::start().await;
    let dir = tempfile::tempdir().unwrap();
    server.put("big.bin", Served { body: content(100_000, 4), ..Served::default() });
    let report = run(vec![DownloadTask::new(server.url("big.bin"), dir.path().join("big.bin")).size(10)])
        .await
        .unwrap();
    assert!(report.failed[0].contains("more than 10 bytes"), "{:?}", report.failed);
    assert!(!dir.path().join("big.bin").exists());
    assert!(leftovers(dir.path()).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn not_enough_space_fails_before_any_request() {
    let server = fake_files::start().await;
    let dir = tempfile::tempdir().unwrap();
    server.put("huge.bin", Served { body: content(10, 0), ..Served::default() });
    let task = DownloadTask::new(server.url("huge.bin"), dir.path().join("huge.bin")).size(u64::MAX / 4);
    assert_eq!(run(vec![task]).await.unwrap_err().code, ErrorCode::NotEnoughSpace);
    assert_eq!(server.total_requests(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn progress_counts_every_byte_and_file() {
    let server = fake_files::start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut tasks = Vec::new();
    for i in 0..10 {
        let body = content(10_240, i as u8);
        server.put(&format!("p{i}"), Served { body: body.clone(), ..Served::default() });
        tasks.push(task(&server, &format!("p{i}"), dir.path(), &body));
    }
    let last = Mutex::new(DownloadProgress::default());
    downloader().download_all(tasks, false, &|p| *last.lock().unwrap() = p).await.unwrap();
    let last = *last.lock().unwrap();
    assert_eq!((last.files_done, last.files_total), (10, 10));
    assert_eq!((last.bytes_done, last.bytes_total), (102_400, 102_400));
}

#[tokio::test(flavor = "multi_thread")]
async fn resumes_after_a_dropped_connection() {
    let server = fake_files::start().await;
    let dir = tempfile::tempdir().unwrap();
    let body = content(200_000, 5);
    server.put(
        "resume.jar",
        Served { body: body.clone(), etag: Some("\"v1\"".into()), drop_first: true, ..Served::default() },
    );
    let report = run(vec![task(&server, "resume.jar", dir.path(), &body)]).await.unwrap();
    assert_eq!(report.downloaded, 1, "{:?}", report.failed);
    assert_eq!(std::fs::read(dir.path().join("resume.jar")).unwrap(), body);
    let seen = server.seen("resume.jar");
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[1].range.as_deref(), Some("bytes=100000-"));
    assert_eq!(seen[1].if_range.as_deref(), Some("\"v1\""));
    assert!(leftovers(dir.path()).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn changed_file_restarts_from_zero() {
    let server = fake_files::start().await;
    let dir = tempfile::tempdir().unwrap();
    let old = content(200_000, 6);
    let new = content(200_000, 9);
    server.put(
        "changed.jar",
        Served {
            body: old,
            etag: Some("\"v1\"".into()),
            drop_first: true,
            replace_after_drop: Some((new.clone(), "\"v2\"".into())),
            ..Served::default()
        },
    );
    let report = run(vec![task(&server, "changed.jar", dir.path(), &new)]).await.unwrap();
    assert_eq!(report.downloaded, 1, "{:?}", report.failed);
    assert_eq!(std::fs::read(dir.path().join("changed.jar")).unwrap(), new, "no old bytes spliced in");
    assert_eq!(server.seen("changed.jar")[1].if_range.as_deref(), Some("\"v1\""));
}

#[tokio::test(flavor = "multi_thread")]
async fn unusable_range_answer_restarts_from_zero() {
    let server = fake_files::start().await;
    let dir = tempfile::tempdir().unwrap();
    let body = content(200_000, 8);
    server.put(
        "odd.jar",
        Served {
            body: body.clone(),
            etag: Some("\"v1\"".into()),
            drop_first: true,
            bad_range: true,
            ..Served::default()
        },
    );
    let report = run(vec![task(&server, "odd.jar", dir.path(), &body)]).await.unwrap();
    assert_eq!(report.downloaded, 1, "{:?}", report.failed);
    assert_eq!(std::fs::read(dir.path().join("odd.jar")).unwrap(), body);
    let seen = server.seen("odd.jar");
    assert_eq!(seen.len(), 3);
    assert!(seen[1].range.is_some() && seen[2].range.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_partial_from_another_request_is_not_reused() {
    let server = fake_files::start().await;
    let dir = tempfile::tempdir().unwrap();
    let body = content(50_000, 10);
    server.put("x.jar", Served { body: body.clone(), etag: Some("\"v1\"".into()), ..Served::default() });
    let stale = dir.path().join("x.jar.part.000000000000000000000000.tmp");
    std::fs::write(&stale, vec![1u8; 1000]).unwrap();
    std::fs::write(
        dir.path().join("x.jar.part.json"),
        r#"{"schema":1,"identity":"{\"url\":\"elsewhere\"}","partial_file":"x.jar.part.000000000000000000000000.tmp","etag":"\"v1\""}"#,
    )
    .unwrap();
    let report = run(vec![task(&server, "x.jar", dir.path(), &body)]).await.unwrap();
    assert_eq!(report.downloaded, 1);
    assert_eq!(server.seen("x.jar")[0].range, None);
    assert!(!stale.exists(), "the stale partial file is removed");
    assert_eq!(std::fs::read(dir.path().join("x.jar")).unwrap(), body);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_ignoring_if_range_is_not_spliced() {
    let server = fake_files::start().await;
    let dir = tempfile::tempdir().unwrap();
    let old = content(200_000, 11);
    let new = content(200_000, 12);
    server.put(
        "moved.jar",
        Served {
            body: old,
            etag: Some("\"v1\"".into()),
            drop_first: true,
            replace_after_drop: Some((new.clone(), "\"v2\"".into())),
            ignore_if_range: true,
            ..Served::default()
        },
    );
    // No hash: only the validator can tell the two versions apart.
    let task = DownloadTask::new(server.url("moved.jar"), dir.path().join("moved.jar")).size(200_000);
    let report = run(vec![task]).await.unwrap();
    assert_eq!(report.downloaded, 1, "{:?}", report.failed);
    assert_eq!(std::fs::read(dir.path().join("moved.jar")).unwrap(), new, "no old bytes spliced in");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_range_restarts_from_zero() {
    let server = fake_files::start().await;
    let dir = tempfile::tempdir().unwrap();
    let body = content(200_000, 13);
    server.put(
        "416.jar",
        Served {
            body: body.clone(),
            etag: Some("\"v1\"".into()),
            drop_first: true,
            range_status: Some(416),
            ..Served::default()
        },
    );
    let report = run(vec![task(&server, "416.jar", dir.path(), &body)]).await.unwrap();
    assert_eq!(report.downloaded, 1, "{:?}", report.failed);
    assert_eq!(std::fs::read(dir.path().join("416.jar")).unwrap(), body);
    assert!(leftovers(dir.path()).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn redirects_to_plain_http_are_refused() {
    let server = fake_files::start().await;
    let dir = tempfile::tempdir().unwrap();
    server.put(
        "hop.jar",
        Served { redirect_to: Some("http://downloads.invalid/hop.jar".into()), ..Served::default() },
    );
    let report =
        run(vec![DownloadTask::new(server.url("hop.jar"), dir.path().join("hop.jar"))]).await.unwrap();
    assert!(report.failed[0].contains("https"), "{:?}", report.failed);
    assert_eq!(server.seen("hop.jar").len(), 1, "not retried");
}

#[tokio::test(flavor = "multi_thread")]
async fn loopback_redirects_are_followed() {
    let server = fake_files::start().await;
    let dir = tempfile::tempdir().unwrap();
    let body = content(5_000, 14);
    server.put("target.jar", Served { body: body.clone(), ..Served::default() });
    server.put("alias.jar", Served { redirect_to: Some(server.url("target.jar")), ..Served::default() });
    let report = run(vec![task(&server, "alias.jar", dir.path(), &body)]).await.unwrap();
    assert_eq!(report.downloaded, 1, "{:?}", report.failed);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_same_file_listed_twice_is_downloaded_once() {
    let server = fake_files::start().await;
    let dir = tempfile::tempdir().unwrap();
    let body = content(80_000, 15);
    server.put("asset", Served { body: body.clone(), ..Served::default() });
    let tasks = vec![task(&server, "asset", dir.path(), &body); 3];
    let report = run(tasks).await.unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!((report.downloaded, report.skipped), (1, 2));
    assert_eq!(server.seen("asset").len(), 1);
    assert_eq!(std::fs::read(dir.path().join("asset")).unwrap(), body);
}

#[tokio::test(flavor = "multi_thread")]
async fn conflicting_tasks_for_one_file_fail() {
    let server = fake_files::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (content(1_000, 16), content(1_000, 17));
    server.put("a.bin", Served { body: a.clone(), ..Served::default() });
    server.put("b.bin", Served { body: b.clone(), ..Served::default() });
    let dest = dir.path().join("same.bin");
    let tasks = vec![
        DownloadTask::new(server.url("a.bin"), &dest).hash(ExpectedHash::sha1(&sha1_hex(&a))),
        DownloadTask::new(server.url("b.bin"), &dest).hash(ExpectedHash::sha1(&sha1_hex(&b))),
    ];
    let report = run(tasks).await.unwrap();
    assert_eq!(report.failed.len(), 2, "{:?}", report.failed);
    assert!(report.failed.iter().all(|f| f.contains("conflicting")), "{:?}", report.failed);
    assert_eq!(server.total_requests(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_small_file_is_fetched_whole_and_started_over() {
    let server = fake_files::start().await;
    let dir = tempfile::tempdir().unwrap();
    let body = content(20_000, 3);
    server.put(
        "small.jar",
        Served { body: body.clone(), etag: Some("\"v1\"".into()), drop_first: true, ..Served::default() },
    );
    let report = run(vec![task(&server, "small.jar", dir.path(), &body)]).await.unwrap();
    assert_eq!(report.downloaded, 1, "{:?}", report.failed);
    assert_eq!(std::fs::read(dir.path().join("small.jar")).unwrap(), body);
    let seen = server.seen("small.jar");
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[1].range, None, "a small file starts over instead of resuming");
    assert!(leftovers(dir.path()).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_leftover_temporary_file_is_written_over() {
    let server = fake_files::start().await;
    let dir = tempfile::tempdir().unwrap();
    let body = content(8_000, 4);
    server.put("left.jar", Served { body: body.clone(), ..Served::default() });
    run(vec![task(&server, "left.jar", dir.path(), &body)]).await.unwrap();
    std::fs::remove_file(dir.path().join("left.jar")).unwrap();
    let names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.is_empty(), "{names:?}");
    // A crash while writing it whole left its temporary file; an earlier, larger version of the
    // file was streamed and stopped half-way (its partial file and sidecar stay).
    std::fs::write(dir.path().join("left.jar.part.whole.tmp"), b"garbage from a crash").unwrap();
    let old_part = "left.jar.part.0123456789abcdef01234567.tmp";
    std::fs::write(dir.path().join(old_part), b"half of an older version").unwrap();
    std::fs::write(
        dir.path().join("left.jar.part.json"),
        serde_json::json!({"schema": 1, "identity": "an older request", "partial_file": old_part})
            .to_string(),
    )
    .unwrap();
    let report = run(vec![task(&server, "left.jar", dir.path(), &body)]).await.unwrap();
    assert_eq!(report.downloaded, 1, "{:?}", report.failed);
    assert_eq!(std::fs::read(dir.path().join("left.jar")).unwrap(), body);
    assert!(leftovers(dir.path()).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_damaged_partial_start_is_caught_by_the_streamed_digest() {
    let server = fake_files::start().await;
    let dir = tempfile::tempdir().unwrap();
    let body = content(200_000, 13);
    server.put(
        "prefix.jar",
        Served { body: body.clone(), etag: Some("\"v1\"".into()), drop_first: true, ..Served::default() },
    );
    // A first run stops half-way: its partial file and sidecar stay.
    let one_try =
        Downloader::new(DownloaderConfig { retries: 1, in_memory_up_to: 32 * 1024, ..fast() }).unwrap();
    let first = one_try
        .download_all(vec![task(&server, "prefix.jar", dir.path(), &body)], false, &|_| {})
        .await
        .unwrap();
    assert_eq!(first.failed.len(), 1);
    let part = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.to_string_lossy().ends_with(".tmp"))
        .unwrap();
    let mut damaged = std::fs::read(&part).unwrap();
    damaged[10] ^= 0xff;
    std::fs::write(&part, damaged).unwrap();
    let report = run(vec![task(&server, "prefix.jar", dir.path(), &body)]).await.unwrap();
    assert_eq!(report.downloaded, 1, "{:?}", report.failed);
    assert_eq!(std::fs::read(dir.path().join("prefix.jar")).unwrap(), body);
    let seen = server.seen("prefix.jar");
    assert_eq!(seen[1].range.as_deref(), Some("bytes=100000-"), "the partial file was resumed");
    assert_eq!(seen[2].range, None, "the digest over both parts failed, so it started over");
    assert!(leftovers(dir.path()).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_digest_that_never_matches_is_counted() {
    let server = fake_files::start().await;
    let dir = tempfile::tempdir().unwrap();
    server.put("bad.jar", Served { body: content(5_000, 2), ..Served::default() });
    server.put("gone.jar", Served { status: Some(404), ..Served::default() });
    let report = run(vec![
        task(&server, "bad.jar", dir.path(), &content(5_000, 1)),
        DownloadTask::new(server.url("gone.jar"), dir.path().join("gone.jar")),
    ])
    .await
    .unwrap();
    assert_eq!((report.failed.len(), report.mismatched), (2, 1));
}

#[tokio::test(flavor = "multi_thread")]
async fn many_present_files_are_checked_and_only_damaged_ones_fetched() {
    let server = fake_files::start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut tasks = Vec::new();
    for i in 0..200 {
        let body = content(1_000 + i, i as u8);
        let name = format!("{:02x}/m{i}.bin", i % 16);
        server.put(&name, Served { body: body.clone(), ..Served::default() });
        tasks.push(
            DownloadTask::new(server.url(&name), dir.path().join(&name))
                .size(body.len() as u64)
                .hash(ExpectedHash::sha1(&sha1_hex(&body))),
        );
    }
    assert_eq!(run(tasks.clone()).await.unwrap().downloaded, 200);
    for i in [3usize, 77, 150] {
        let path = dir.path().join(format!("{:02x}/m{i}.bin", i % 16));
        std::fs::write(&path, vec![0u8; 1_000 + i]).unwrap();
    }
    let repaired = downloader().download_all(tasks, true, &|_| {}).await.unwrap();
    assert_eq!((repaired.downloaded, repaired.skipped, repaired.failed.len()), (3, 197, 0));
    assert_eq!(server.total_requests(), 203);
}

#[tokio::test(flavor = "multi_thread")]
async fn one_file_names_its_own_failure() {
    let server = fake_files::start().await;
    let dir = tempfile::tempdir().unwrap();
    server.put("bad.jar", Served { body: content(5_000, 2), ..Served::default() });
    server.put("gone.jar", Served { status: Some(404), ..Served::default() });
    let bad =
        downloader().download_one(task(&server, "bad.jar", dir.path(), &content(5_000, 1)), &|_| {}).await;
    assert_eq!(bad.unwrap_err().code, ErrorCode::IntegrityMismatch);
    let gone = downloader()
        .download_one(DownloadTask::new(server.url("gone.jar"), dir.path().join("gone.jar")), &|_| {})
        .await;
    assert_eq!(gone.unwrap_err().code, ErrorCode::DownloadFailed);
    assert!(leftovers(dir.path()).is_empty());
}

/// A credential for the hosts on `server`'s port: the `x-test-key` header with `secret`.
fn credential_for(server: &FileServer, secret: &str) -> Credential {
    let port = reqwest::Url::parse(&server.base).unwrap().port();
    let mut value = reqwest::header::HeaderValue::from_str(secret).unwrap();
    value.set_sensitive(true);
    Credential::new(
        reqwest::header::HeaderName::from_static("x-test-key"),
        value,
        move |url: &reqwest::Url| url.port() == port,
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn a_credential_goes_to_its_hosts_only() {
    let home = fake_files::start().await;
    let other = fake_files::start().await;
    let dir = tempfile::tempdir().unwrap();
    for (name, body) in [("small.jar", content(5_000, 31)), ("large.jar", content(100_000, 32))] {
        home.put(name, Served { body: body.clone(), ..Served::default() });
        other.put(name, Served { body: body.clone(), ..Served::default() });
        let keyed = task(&home, name, dir.path(), &body).credential(credential_for(&home, "secret-1"));
        assert!(!format!("{keyed:?}").contains("secret-1"), "the task's Debug hides the key");
        run(vec![keyed]).await.unwrap().into_result().unwrap();
        assert_eq!(home.seen(name)[0].key.as_deref(), Some("secret-1"), "{name}: its host gets the key");
        std::fs::remove_file(dir.path().join(name)).unwrap();
        // The same credential on another host's address: no key there.
        let elsewhere = task(&other, name, dir.path(), &body).credential(credential_for(&home, "secret-1"));
        run(vec![elsewhere]).await.unwrap().into_result().unwrap();
        assert_eq!(other.seen(name)[0].key, None, "{name}: another host never gets it");
        std::fs::remove_file(dir.path().join(name)).unwrap();
        // No credential, no key.
        run(vec![task(&home, name, dir.path(), &body)]).await.unwrap().into_result().unwrap();
        assert_eq!(home.seen(name).last().unwrap().key, None, "{name}: a plain task sends none");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_credential_never_follows_a_redirect_away() {
    let home = fake_files::start().await;
    let elsewhere = fake_files::start().await;
    let dir = tempfile::tempdir().unwrap();
    for (name, body) in [("small.jar", content(5_000, 33)), ("large.jar", content(100_000, 34))] {
        elsewhere.put(name, Served { body: body.clone(), ..Served::default() });
        home.put(name, Served { redirect_to: Some(elsewhere.url(name)), ..Served::default() });
        let keyed = task(&home, name, dir.path(), &body).credential(credential_for(&home, "secret-2"));
        let report = run(vec![keyed]).await.unwrap();
        assert_eq!(report.failed.len(), 1, "{name}");
        assert!(elsewhere.seen(name).is_empty(), "{name}: the other host is never asked");
        assert!(!dir.path().join(name).exists(), "{name}");
    }
    assert!(leftovers(dir.path()).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_download_ending_outside_its_origin_is_refused() {
    let home = fake_files::start().await;
    let elsewhere = fake_files::start().await;
    let dir = tempfile::tempdir().unwrap();
    for (name, body) in [("small.jar", content(5_000, 21)), ("large.jar", content(100_000, 22))] {
        elsewhere.put(name, Served { body: body.clone(), ..Served::default() });
        home.put(name, Served { redirect_to: Some(elsewhere.url(name)), ..Served::default() });
        let port = reqwest::Url::parse(&home.base).unwrap().port();
        let origin = Origin::new(move |url: &reqwest::Url| url.port() == port);
        let report = run(vec![task(&home, name, dir.path(), &body).origin(origin)]).await.unwrap();
        assert_eq!(report.failed.len(), 1, "{name}");
        assert!(report.failed[0].contains("not allowed"), "{:?}", report.failed);
        assert!(!dir.path().join(name).exists(), "{name}");
    }
    assert!(leftovers(dir.path()).is_empty());
}
