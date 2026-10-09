//! Whether a newer Postvak release is out: GitHub's latest release, asked at most once a day. Postvak never updates
//! itself; the window only links to the release notes.
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::paths::write_atomic;

const DAY: i64 = 24 * 60 * 60;
const HOST: &str = "api.github.com";
const LATEST: &str = "/repos/postvak-app/postvak/releases/latest";
const TIMEOUT: Duration = Duration::from_secs(10);

/// The last check: when it ran, and the newest stable version it saw as `X.Y.Z`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cached {
    pub checked_at: i64,
    pub latest: Option<String>,
}

#[derive(Deserialize)]
struct Release {
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    tag_name: String,
}

/// `X.Y.Z` with or without a leading `v`; anything else, prereleases included, is None.
pub fn parse_version(text: &str) -> Option<[u64; 3]> {
    let text = text.strip_prefix('v').unwrap_or(text);
    let mut parts = text.split('.').map(|part| part.parse::<u64>().ok());
    let version = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(version)
}

/// The stable version in a `releases/latest` reply, rebuilt from its numbers so nothing else reaches a URL.
pub fn latest_from_json(json: &str) -> Option<String> {
    let release: Release = serde_json::from_str(json).ok()?;
    if release.draft || release.prerelease {
        return None;
    }
    let [major, minor, patch] = parse_version(&release.tag_name)?;
    Some(format!("{major}.{minor}.{patch}"))
}

pub fn newer(latest: &str, current: &str) -> bool {
    matches!((parse_version(latest), parse_version(current)), (Some(latest), Some(current)) if latest > current)
}

pub fn release_url(version: &str) -> String {
    format!("https://github.com/postvak-app/postvak/releases/tag/v{version}")
}

/// Due with no earlier check, after a day, or when the clock went back past the last one.
pub fn due(cached: Option<&Cached>, now: i64) -> bool {
    cached.is_none_or(|cached| now - cached.checked_at >= DAY || cached.checked_at > now)
}

pub fn load(path: &Path) -> Option<Cached> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

pub fn save(path: &Path, cached: &Cached) -> io::Result<()> {
    write_atomic(path, serde_json::to_string(cached)?.as_bytes())
}

/// The version to announce, if one newer than `current` is out. Calls `fetch` only when a check is due; a failed fetch
/// still counts as the day's check and keeps the version seen before.
pub fn check(
    cache: &Path,
    now: i64,
    current: &str,
    fetch: fn() -> anyhow::Result<String>,
) -> Option<String> {
    let cached = load(cache);
    let latest = if due(cached.as_ref(), now) {
        let latest = match fetch() {
            Ok(body) => latest_from_json(&body),
            Err(e) => {
                log::debug!("update check failed: {e:#}");
                cached.and_then(|cached| cached.latest)
            }
        };
        let fresh = Cached {
            checked_at: now,
            latest: latest.clone(),
        };
        if let Err(e) = save(cache, &fresh) {
            log::debug!("could not save the update check: {e}");
        }
        latest
    } else {
        cached.and_then(|cached| cached.latest)
    };
    latest.filter(|latest| newer(latest, current))
}

/// Asks GitHub for the latest release over HTTP/1.0, which rules out a chunked reply.
pub fn fetch() -> anyhow::Result<String> {
    let tcp = TcpStream::connect((HOST, 443))?;
    tcp.set_read_timeout(Some(TIMEOUT))?;
    tcp.set_write_timeout(Some(TIMEOUT))?;
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from(HOST)?;
    let mut tls =
        rustls::StreamOwned::new(rustls::ClientConnection::new(Arc::new(config), name)?, tcp);
    let request = format!(
        "GET {LATEST} HTTP/1.0\r\nHost: {HOST}\r\nUser-Agent: postvak/{}\r\nAccept: application/vnd.github+json\r\n\r\n",
        env!("CARGO_PKG_VERSION")
    );
    tls.write_all(request.as_bytes())?;
    let mut response = Vec::new();
    match tls.read_to_end(&mut response) {
        Ok(_) => {}
        // A server that closes without TLS close_notify; a cut-off body then fails to parse as JSON.
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof && !response.is_empty() => {}
        Err(e) => return Err(e.into()),
    }
    Ok(body_of(&String::from_utf8(response)?)?.to_string())
}

/// The body of a 200 reply; any other status is an error.
pub fn body_of(response: &str) -> anyhow::Result<&str> {
    let (head, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| anyhow::anyhow!("no end of headers"))?;
    let status = head.lines().next().unwrap_or_default();
    anyhow::ensure!(
        status.split(' ').nth(1) == Some("200"),
        "GitHub answered {status}"
    );
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY_AGO: i64 = 1_790_000_000;
    const NOW: i64 = DAY_AGO + DAY;

    fn release(tag: &str, draft: bool, prerelease: bool) -> String {
        format!(
            r#"{{"html_url":"https://github.com/postvak-app/postvak/releases/tag/{tag}","tag_name":"{tag}","draft":{draft},"prerelease":{prerelease},"assets":[]}}"#
        )
    }

    fn release_0_3_0() -> anyhow::Result<String> {
        Ok(release("v0.3.0", false, false))
    }

    fn release_5_0_0() -> anyhow::Result<String> {
        Ok(release("v5.0.0", false, false))
    }

    fn offline() -> anyhow::Result<String> {
        Err(anyhow::anyhow!("offline"))
    }

    fn cache_in(dir: &tempfile::TempDir) -> std::path::PathBuf {
        crate::paths::Paths::under(dir.path()).update_check()
    }

    #[test]
    fn parse_version_takes_plain_versions_with_or_without_v() {
        assert_eq!(parse_version("v0.3.0"), Some([0, 3, 0]));
        assert_eq!(parse_version("1.12.7"), Some([1, 12, 7]));
        assert_eq!(parse_version("v0.3.0-rc.1"), None);
        assert_eq!(parse_version("v1.2"), None);
        assert_eq!(parse_version("v1.2.3.4"), None);
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version(" v1.2.3"), None);
    }

    #[test]
    fn newer_compares_numbers_not_strings() {
        assert!(newer("0.10.0", "0.9.0"));
        assert!(newer("1.0.0", "0.99.99"));
        assert!(!newer("0.2.0", "0.2.0"));
        assert!(!newer("0.1.9", "0.2.0"));
        assert!(!newer("garbage", "0.2.0"));
    }

    #[test]
    fn only_stable_plain_versions_are_announced() {
        assert_eq!(
            latest_from_json(&release("v0.3.0", false, false)).as_deref(),
            Some("0.3.0")
        );
        assert_eq!(
            latest_from_json(&release("0.3.0", false, false)).as_deref(),
            Some("0.3.0")
        );
        assert_eq!(latest_from_json(&release("v0.3.0", true, false)), None);
        assert_eq!(latest_from_json(&release("v0.3.0", false, true)), None);
        assert_eq!(
            latest_from_json(&release("v0.3.0-rc.1", false, false)),
            None
        );
        assert_eq!(
            latest_from_json(&release("v1.2.3/../../evil", false, false)),
            None
        );
        assert_eq!(
            latest_from_json(&release(
                "v123456789012345678901234567890.0.0",
                false,
                false
            )),
            None
        );
        assert_eq!(
            latest_from_json(r#"{"message":"API rate limit exceeded"}"#),
            None
        );
        assert_eq!(latest_from_json("<html>"), None);
        assert_eq!(
            release_url("0.3.0"),
            "https://github.com/postvak-app/postvak/releases/tag/v0.3.0"
        );
    }

    #[test]
    fn due_when_missing_old_or_in_the_future() {
        let cached = |checked_at| Cached {
            checked_at,
            latest: None,
        };
        assert!(due(None, NOW));
        assert!(!due(Some(&cached(NOW - 60 * 60)), NOW));
        assert!(due(Some(&cached(DAY_AGO)), NOW));
        assert!(
            due(Some(&cached(NOW + 60)), NOW),
            "a clock that went back must not block checks"
        );
    }

    #[test]
    fn a_fresh_cache_answers_without_fetching() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_in(&dir);
        save(
            &cache,
            &Cached {
                checked_at: NOW - 60,
                latest: Some("0.3.0".into()),
            },
        )
        .unwrap();
        assert_eq!(
            check(&cache, NOW, "0.2.0", release_5_0_0).as_deref(),
            Some("0.3.0")
        );
    }

    #[test]
    fn a_due_check_fetches_and_caches_the_result() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_in(&dir);
        assert_eq!(
            check(&cache, NOW, "0.2.0", release_0_3_0).as_deref(),
            Some("0.3.0")
        );
        assert_eq!(
            load(&cache),
            Some(Cached {
                checked_at: NOW,
                latest: Some("0.3.0".into())
            })
        );
        assert_eq!(
            check(&cache, NOW + 60, "0.2.0", release_5_0_0).as_deref(),
            Some("0.3.0")
        );
        assert_eq!(
            check(&cache, NOW + DAY, "0.2.0", release_5_0_0).as_deref(),
            Some("5.0.0")
        );
    }

    #[test]
    fn a_failed_fetch_counts_as_the_check_and_keeps_the_last_version() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_in(&dir);
        assert_eq!(check(&cache, NOW, "0.2.0", offline), None);
        assert_eq!(
            load(&cache),
            Some(Cached {
                checked_at: NOW,
                latest: None
            })
        );
        save(
            &cache,
            &Cached {
                checked_at: DAY_AGO,
                latest: Some("0.3.0".into()),
            },
        )
        .unwrap();
        assert_eq!(
            check(&cache, NOW, "0.2.0", offline).as_deref(),
            Some("0.3.0")
        );
        assert_eq!(
            load(&cache),
            Some(Cached {
                checked_at: NOW,
                latest: Some("0.3.0".into())
            })
        );
    }

    #[test]
    fn a_cached_version_the_user_already_runs_is_not_announced() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_in(&dir);
        save(
            &cache,
            &Cached {
                checked_at: NOW - 60,
                latest: Some("0.3.0".into()),
            },
        )
        .unwrap();
        assert_eq!(check(&cache, NOW, "0.3.0", release_5_0_0), None);
        assert_eq!(check(&cache, NOW, "0.4.0", release_5_0_0), None);
    }

    #[test]
    fn a_corrupt_cache_file_means_a_new_check() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_in(&dir);
        crate::paths::write_atomic(&cache, b"{not json").unwrap();
        assert_eq!(load(&cache), None);
        assert_eq!(
            check(&cache, NOW, "0.2.0", release_0_3_0).as_deref(),
            Some("0.3.0")
        );
    }

    #[test]
    fn body_of_takes_a_200_body_and_rejects_other_statuses() {
        let ok = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nconnection: close\r\n\r\n{\"tag_name\":\"v0.3.0\"}";
        assert_eq!(body_of(ok).unwrap(), "{\"tag_name\":\"v0.3.0\"}");
        let limited =
            "HTTP/1.1 403 rate limit exceeded\r\n\r\n{\"message\":\"API rate limit exceeded\"}";
        assert!(body_of(limited).unwrap_err().to_string().contains("403"));
        assert!(body_of("HTTP/1.1 200 OK\r\nContent-Type: application/json").is_err());
        assert!(body_of("").is_err());
    }
}
