//! Host names for every web request: the computer's own DNS first; when it fails or stays
//! silent, Cloudflare's and Google's DNS over HTTPS, which an ISP can neither answer for nor
//! block. A host the system could not find goes straight to them for the rest of the run.

use std::collections::HashSet;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

/// A way to find a host's addresses (their ports are 0: the request's own port is used).
pub type Lookup = Arc<
    dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<Vec<SocketAddr>, String>> + Send>> + Send + Sync,
>;

/// How long the system's DNS gets before the public ones are asked.
const SYSTEM_WAIT: Duration = Duration::from_secs(4);

/// The system's DNS, then the public ones over HTTPS.
#[derive(Clone)]
pub struct FallbackDns {
    system: Lookup,
    public: Lookup,
    wait: Duration,
    /// Hosts the system could not find: asked of the public DNS at once.
    failing: Arc<Mutex<HashSet<String>>>,
}

impl FallbackDns {
    pub fn new(system: Lookup, public: Lookup, wait: Duration) -> FallbackDns {
        FallbackDns { system, public, wait, failing: Arc::default() }
    }

    /// `host`'s addresses: the system's answer, else the public DNS's.
    pub async fn lookup(&self, host: &str) -> Result<Vec<SocketAddr>, String> {
        let known = self.failing.lock().unwrap_or_else(|e| e.into_inner()).contains(host);
        let mut system_error = String::from("skipped: it failed before");
        if !known {
            system_error = match tokio::time::timeout(self.wait, (self.system)(host.to_string())).await {
                Ok(Ok(addrs)) if !addrs.is_empty() => return Ok(addrs),
                Ok(Ok(_)) => "no address".to_string(),
                Ok(Err(e)) => e,
                Err(_) => format!("no answer in {:?}", self.wait),
            };
        }
        match (self.public)(host.to_string()).await {
            Ok(addrs) if !addrs.is_empty() => {
                if !known && self.failing.lock().unwrap_or_else(|e| e.into_inner()).insert(host.to_string()) {
                    tracing::warn!(
                        "The system's DNS could not find {host} ({system_error}); public DNS over HTTPS did"
                    );
                }
                Ok(addrs)
            }
            Ok(_) => Err(format!("{host}: the system's DNS: {system_error}; public DNS: no address")),
            Err(e) => Err(format!("{host}: the system's DNS: {system_error}; public DNS: {e}")),
        }
    }
}

impl Resolve for FallbackDns {
    fn resolve(&self, name: Name) -> Resolving {
        let this = self.clone();
        Box::pin(async move {
            let addrs = this.lookup(name.as_str()).await?;
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

/// The computer's own DNS, as every program uses it.
fn system() -> Lookup {
    Arc::new(|host| {
        Box::pin(async move {
            tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map(Iterator::collect)
                .map_err(|e| e.to_string())
        })
    })
}

/// Cloudflare's and Google's DNS over HTTPS, reached by their addresses (no DNS needed for them).
fn public() -> Lookup {
    use hickory_resolver::config::{CLOUDFLARE, GOOGLE, ResolverConfig};
    use hickory_resolver::net::runtime::TokioRuntimeProvider;
    static RESOLVER: OnceLock<Result<hickory_resolver::TokioResolver, String>> = OnceLock::new();
    Arc::new(|host| {
        Box::pin(async move {
            let resolver = RESOLVER
                .get_or_init(|| {
                    let _ = rustls::crypto::ring::default_provider().install_default();
                    let mut config = ResolverConfig::https(&CLOUDFLARE);
                    GOOGLE.https().for_each(|server| config.add_name_server(server));
                    hickory_resolver::TokioResolver::builder_with_config(
                        config,
                        TokioRuntimeProvider::default(),
                    )
                    .build()
                    .map_err(|e| e.to_string())
                })
                .as_ref()
                .map_err(Clone::clone)?;
            let found = resolver.lookup_ip(host.as_str()).await.map_err(|e| e.to_string())?;
            Ok(found.iter().map(|ip| SocketAddr::new(ip, 0)).collect())
        })
    })
}

/// The one resolver every client of the launcher shares (and with it what it learnt).
pub fn shared() -> FallbackDns {
    static SHARED: OnceLock<FallbackDns> = OnceLock::new();
    SHARED.get_or_init(|| FallbackDns::new(system(), public(), SYSTEM_WAIT)).clone()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn addr(last: u8) -> SocketAddr {
        SocketAddr::from(([10, 0, 0, last], 0))
    }

    /// A lookup that counts its calls and answers `answer` (`None`: never answers).
    fn fake(calls: &Arc<AtomicUsize>, answer: Option<Result<Vec<SocketAddr>, String>>) -> Lookup {
        let calls = calls.clone();
        Arc::new(move |_| {
            calls.fetch_add(1, Ordering::Relaxed);
            let answer = answer.clone();
            Box::pin(async move {
                match answer {
                    Some(answer) => answer,
                    None => std::future::pending().await,
                }
            })
        })
    }

    #[tokio::test]
    async fn the_system_s_answer_is_used_when_it_has_one() {
        let (system, public) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let dns = FallbackDns::new(
            fake(&system, Some(Ok(vec![addr(1)]))),
            fake(&public, Some(Ok(vec![addr(2)]))),
            Duration::from_secs(1),
        );
        assert_eq!(dns.lookup("example.net").await, Ok(vec![addr(1)]));
        assert_eq!((system.load(Ordering::Relaxed), public.load(Ordering::Relaxed)), (1, 0));
    }

    #[tokio::test]
    async fn a_host_the_system_cannot_find_is_found_by_the_public_dns_from_then_on() {
        let (system, public) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let dns = FallbackDns::new(
            fake(&system, Some(Err("no such host".into()))),
            fake(&public, Some(Ok(vec![addr(2)]))),
            Duration::from_secs(1),
        );
        assert_eq!(dns.lookup("example.net").await, Ok(vec![addr(2)]));
        assert_eq!(dns.lookup("example.net").await, Ok(vec![addr(2)]));
        assert_eq!(system.load(Ordering::Relaxed), 1, "the system is not asked again for that host");
        assert_eq!(public.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn a_silent_system_dns_is_not_waited_for_long() {
        let (system, public) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let dns = FallbackDns::new(
            fake(&system, None),
            fake(&public, Some(Ok(vec![addr(2)]))),
            Duration::from_millis(50),
        );
        assert_eq!(dns.lookup("example.net").await, Ok(vec![addr(2)]));
    }

    #[tokio::test]
    async fn when_both_fail_both_reasons_are_told() {
        let (system, public) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let dns = FallbackDns::new(
            fake(&system, Some(Err("no such host".into()))),
            fake(&public, Some(Err("connection refused".into()))),
            Duration::from_secs(1),
        );
        let error = dns.lookup("example.net").await.unwrap_err();
        assert!(error.contains("no such host") && error.contains("connection refused"), "{error}");
    }

    /// Asks Cloudflare and Google for real: run by hand with `--ignored`.
    #[tokio::test]
    #[ignore = "needs the internet"]
    async fn the_public_dns_answers_over_https() {
        let found = (public())("resources.download.minecraft.net".into()).await.unwrap();
        assert!(!found.is_empty());
    }
}
