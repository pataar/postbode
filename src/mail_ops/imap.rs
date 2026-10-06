use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_imap::Session;
use async_imap::extensions::idle::IdleResponse;
use async_imap::types::{Fetch, Flag, NameAttribute};
use futures_util::TryStreamExt;
use rustls::pki_types::{CertificateDer, pem::PemObject};
use tokio::net::TcpStream;
use tokio::runtime::Runtime;
use tokio_rustls::client::TlsStream;

use crate::config::AccountConfig;
use crate::credentials::Secret;
use crate::mail_ops::{
    Envelope, FlagUpdate, IdleOutcome, MailError, MailOps, MailResult, RemoteFolder, SelectInfo,
};

type ImapSession = Session<TlsStream<TcpStream>>;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// A peer that vanished (a NAT entry dropped during sleep) is noticed after about 60 + 4 × 15 seconds instead of never.
const KEEPALIVE_IDLE: Duration = Duration::from_secs(60);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
const KEEPALIVE_RETRIES: u32 = 4;

pub struct ImapOps {
    rt: Runtime,
    session: Option<ImapSession>,
    has_idle: bool,
    has_move: bool,
    has_uidplus: bool,
}

impl ImapOps {
    pub fn connect(account: &AccountConfig, secret: &Secret) -> MailResult<ImapOps> {
        ImapOps::connect_within(account, secret, CONNECT_TIMEOUT)
    }

    // ponytail: only connecting is bounded; a live server that stops answering mid-command still blocks. Wrap block_on in a timeout if that shows up.
    fn connect_within(
        account: &AccountConfig,
        secret: &Secret,
        limit: Duration,
    ) -> MailResult<ImapOps> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| MailError::Io(e.to_string()))?;
        let opened = rt.block_on(async {
            tokio::time::timeout(limit, open_session(account, secret))
                .await
                .map_err(|_| {
                    MailError::Connect(format!(
                        "{}:{} did not answer within {limit:?}",
                        account.host, account.port
                    ))
                })?
        });
        let (session, has_idle, has_move, has_uidplus) = match opened {
            Ok(opened) => opened,
            Err(error) => {
                // Dropping the runtime would wait for a blocking DNS lookup past the limit.
                rt.shutdown_background();
                return Err(error);
            }
        };
        Ok(ImapOps {
            rt,
            session: Some(session),
            has_idle,
            has_move,
            has_uidplus,
        })
    }

    /// Splits the borrow so `rt.block_on` can drive a future that holds the session.
    fn parts(&mut self) -> MailResult<(&Runtime, &mut ImapSession)> {
        let session = self
            .session
            .as_mut()
            .ok_or_else(|| MailError::Io("session closed".into()))?;
        Ok((&self.rt, session))
    }

    pub fn has_move(&self) -> bool {
        self.has_move
    }

    pub fn delete_folder(&mut self, name: &str) -> MailResult<()> {
        let (rt, session) = self.parts()?;
        rt.block_on(async { session.delete(name).await.map_err(proto) })
    }
}

/// The public web roots, plus the account's own CA when it sets `ca_file`.
fn root_store(ca_file: Option<&Path>) -> MailResult<rustls::RootCertStore> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let Some(path) = ca_file else {
        return Ok(roots);
    };
    let invalid =
        |reason: String| MailError::Connect(format!("ca_file {}: {reason}", path.display()));
    let certs = CertificateDer::pem_file_iter(path)
        .and_then(|certs| certs.collect::<Result<Vec<_>, _>>())
        .map_err(|e| invalid(e.to_string()))?;
    let (added, _) = roots.add_parsable_certificates(certs);
    if added == 0 {
        return Err(invalid("no certificate found".into()));
    }
    Ok(roots)
}

async fn open_session(
    account: &AccountConfig,
    secret: &Secret,
) -> MailResult<(ImapSession, bool, bool, bool)> {
    let tcp = TcpStream::connect((account.host.as_str(), account.port))
        .await
        .map_err(|e| MailError::Connect(e.to_string()))?;
    enable_keepalive(&tcp).map_err(|e| MailError::Connect(e.to_string()))?;
    let roots = root_store(account.ca_file.as_deref())?;
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    let name = rustls::pki_types::ServerName::try_from(account.host.clone())
        .map_err(|e| MailError::Connect(e.to_string()))?;
    let tls = connector
        .connect(name, tcp)
        .await
        .map_err(|e| MailError::Connect(e.to_string()))?;
    let client = async_imap::Client::new(tls);
    let mut session = client
        .login(&account.username, secret.expose())
        .await
        .map_err(|(e, _)| MailError::Auth(e.to_string()))?;
    let caps = session.capabilities().await.map_err(proto)?;
    let has_idle = caps.has_str("IDLE");
    let has_move = caps.has_str("MOVE");
    let has_uidplus = caps.has_str("UIDPLUS");
    Ok((session, has_idle, has_move, has_uidplus))
}

fn enable_keepalive(tcp: &TcpStream) -> std::io::Result<()> {
    let params = socket2::TcpKeepalive::new()
        .with_time(KEEPALIVE_IDLE)
        .with_interval(KEEPALIVE_INTERVAL)
        .with_retries(KEEPALIVE_RETRIES);
    socket2::SockRef::from(tcp).set_tcp_keepalive(&params)
}

fn proto(e: async_imap::error::Error) -> MailError {
    match e {
        async_imap::error::Error::Io(e) => MailError::Io(e.to_string()),
        other => MailError::Protocol(other.to_string()),
    }
}

fn flag_to_string(flag: &Flag<'_>) -> String {
    match flag {
        Flag::Seen => "\\Seen".into(),
        Flag::Answered => "\\Answered".into(),
        Flag::Flagged => "\\Flagged".into(),
        Flag::Deleted => "\\Deleted".into(),
        Flag::Draft => "\\Draft".into(),
        Flag::Recent => "\\Recent".into(),
        Flag::MayCreate => "\\*".into(),
        Flag::Custom(s) => s.to_string(),
    }
}

fn special_use(attrs: &[NameAttribute<'_>]) -> Option<String> {
    attrs.iter().find_map(|a| match a {
        NameAttribute::Trash => Some("Trash".into()),
        NameAttribute::Sent => Some("Sent".into()),
        NameAttribute::Junk => Some("Junk".into()),
        NameAttribute::Drafts => Some("Drafts".into()),
        NameAttribute::Archive => Some("Archive".into()),
        NameAttribute::Extension(s) => match s.trim_start_matches('\\') {
            x @ ("Trash" | "Sent" | "Junk" | "Drafts" | "Archive") => Some(x.to_string()),
            _ => None,
        },
        _ => None,
    })
}

/// None for partial responses such as an unsolicited FLAGS-only FETCH, which would otherwise store an empty message.
fn envelope_from(fetch: &Fetch) -> Option<Envelope> {
    Some(Envelope {
        uid: fetch.uid?,
        flags: fetch.flags().map(|f| flag_to_string(&f)).collect(),
        internaldate: fetch.internal_date()?.timestamp(),
        size: fetch.size,
        headers: fetch.header()?.to_vec(),
    })
}

impl MailOps for ImapOps {
    fn list_folders(&mut self) -> MailResult<Vec<RemoteFolder>> {
        let (rt, session) = self.parts()?;
        rt.block_on(async {
            let names: Vec<_> = session
                .list(Some(""), Some("*"))
                .await
                .map_err(proto)?
                .try_collect()
                .await
                .map_err(proto)?;
            Ok(names
                .iter()
                .filter(|n| {
                    !n.attributes()
                        .iter()
                        .any(|a| matches!(a, NameAttribute::NoSelect))
                })
                .map(|n| RemoteFolder {
                    name: n.name().to_string(),
                    special_use: special_use(n.attributes()),
                })
                .collect())
        })
    }

    fn select(&mut self, folder: &str) -> MailResult<SelectInfo> {
        let (rt, session) = self.parts()?;
        rt.block_on(async {
            let mailbox = session.select(folder).await.map_err(proto)?;
            Ok(SelectInfo {
                uidvalidity: mailbox.uid_validity.unwrap_or(0),
            })
        })
    }

    fn search_uids(&mut self, from_uid: u32) -> MailResult<Vec<u32>> {
        let (rt, session) = self.parts()?;
        rt.block_on(async {
            let found = session
                .uid_search(format!("UID {from_uid}:*"))
                .await
                .map_err(proto)?;
            let mut uids: Vec<u32> = found.into_iter().collect();
            uids.sort_unstable();
            Ok(uids)
        })
    }

    fn fetch_envelopes(&mut self, first: u32, last: u32) -> MailResult<Vec<Envelope>> {
        let (rt, session) = self.parts()?;
        rt.block_on(async {
            let fetches: Vec<Fetch> = session
                .uid_fetch(
                    format!("{first}:{last}"),
                    "(UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER])",
                )
                .await
                .map_err(proto)?
                .try_collect()
                .await
                .map_err(proto)?;
            Ok(fetches
                .iter()
                .filter_map(envelope_from)
                .filter(|e| (first..=last).contains(&e.uid))
                .collect())
        })
    }

    fn fetch_flags(&mut self, upto_uid: u32) -> MailResult<Vec<FlagUpdate>> {
        if upto_uid == 0 {
            return Ok(vec![]);
        }
        let (rt, session) = self.parts()?;
        rt.block_on(async {
            let fetches: Vec<Fetch> = session
                .uid_fetch(format!("1:{upto_uid}"), "(UID FLAGS)")
                .await
                .map_err(proto)?
                .try_collect()
                .await
                .map_err(proto)?;
            Ok(fetches
                .iter()
                .filter_map(|f| {
                    Some(FlagUpdate {
                        uid: f.uid?,
                        flags: f.flags().map(|x| flag_to_string(&x)).collect(),
                    })
                })
                .collect())
        })
    }

    fn fetch_raw(&mut self, uid: u32) -> MailResult<Option<Vec<u8>>> {
        let (rt, session) = self.parts()?;
        rt.block_on(async {
            let fetches: Vec<Fetch> = session
                .uid_fetch(uid.to_string(), "(UID BODY.PEEK[])")
                .await
                .map_err(proto)?
                .try_collect()
                .await
                .map_err(proto)?;
            Ok(fetches
                .iter()
                .find(|f| f.uid == Some(uid))
                .and_then(|f| f.body().map(<[u8]>::to_vec)))
        })
    }

    fn add_flags(&mut self, uid: u32, flags: &[&str]) -> MailResult<()> {
        store_flags(self, uid, '+', flags)
    }

    fn remove_flags(&mut self, uid: u32, flags: &[&str]) -> MailResult<()> {
        store_flags(self, uid, '-', flags)
    }

    fn expunge(&mut self, uid: u32) -> MailResult<()> {
        let has_uidplus = self.has_uidplus;
        let (rt, session) = self.parts()?;
        rt.block_on(async {
            if has_uidplus {
                let _: Vec<_> = session
                    .uid_expunge(uid.to_string())
                    .await
                    .map_err(proto)?
                    .try_collect()
                    .await
                    .map_err(proto)?;
            } else {
                let _: Vec<_> = session
                    .expunge()
                    .await
                    .map_err(proto)?
                    .try_collect()
                    .await
                    .map_err(proto)?;
            }
            Ok(())
        })
    }

    fn move_message(&mut self, uid: u32, to: &str) -> MailResult<Option<u32>> {
        let has_move = self.has_move;
        let (rt, session) = self.parts()?;
        if has_move {
            rt.block_on(async { session.uid_mv(uid.to_string(), to).await.map_err(proto) })?;
            return Ok(None);
        }
        rt.block_on(async { session.uid_copy(uid.to_string(), to).await.map_err(proto) })?;
        self.add_flags(uid, &["\\Deleted"])?;
        self.expunge(uid)?;
        Ok(None)
    }

    fn create_folder(&mut self, name: &str) -> MailResult<()> {
        let (rt, session) = self.parts()?;
        rt.block_on(async { session.create(name).await.map_err(proto) })
    }

    fn append(&mut self, folder: &str, raw: &[u8], flags: &[&str]) -> MailResult<()> {
        let flags = (!flags.is_empty()).then(|| format!("({})", flags.join(" ")));
        let (rt, session) = self.parts()?;
        rt.block_on(async {
            session
                .append(folder, flags.as_deref(), None, raw)
                .await
                .map_err(proto)
        })
    }

    fn idle(&mut self, timeout: Duration, interrupt: &AtomicBool) -> MailResult<IdleOutcome> {
        if interrupt.load(Ordering::Relaxed) {
            return Ok(IdleOutcome::Interrupted);
        }
        if !self.has_idle {
            return Ok(sleep_until(timeout, interrupt));
        }
        let timeout = timeout.min(Duration::from_secs(29 * 60));
        let session = self
            .session
            .take()
            .ok_or_else(|| MailError::Io("session closed".into()))?;
        let (outcome, session) = self.rt.block_on(async {
            let mut handle = session.idle();
            handle.init().await.map_err(proto)?;
            // async-imap's timeout only bounds silence; server keepalives reset it, so also keep an absolute deadline.
            let (wait, stop) = handle.wait_with_timeout(timeout);
            let mut stop = Some(stop);
            let deadline = tokio::time::sleep(timeout);
            tokio::pin!(deadline);
            let outcome = {
                let mut wait = std::pin::pin!(wait);
                loop {
                    tokio::select! {
                        res = &mut wait => break match res.map_err(proto)? {
                            IdleResponse::NewData(_) => IdleOutcome::NewMail,
                            IdleResponse::Timeout => IdleOutcome::Timeout,
                            IdleResponse::ManualInterrupt => IdleOutcome::Interrupted,
                        },
                        _ = &mut deadline => {
                            drop(stop.take());
                            break IdleOutcome::Timeout;
                        },
                        _ = tokio::time::sleep(Duration::from_millis(500)) => {
                            if interrupt.load(Ordering::Relaxed) {
                                // Dropping the StopSource makes `wait` resolve with ManualInterrupt.
                                drop(stop.take());
                            }
                        }
                    }
                }
            };
            let session = handle.done().await.map_err(proto)?;
            Ok::<_, MailError>((outcome, session))
        })?;
        self.session = Some(session);
        Ok(outcome)
    }
}

/// The timer loop for servers without IDLE: sleeps in short ticks so shutdown stays responsive.
fn sleep_until(timeout: Duration, interrupt: &AtomicBool) -> IdleOutcome {
    let deadline = Instant::now() + timeout;
    loop {
        if interrupt.load(Ordering::Relaxed) {
            return IdleOutcome::Interrupted;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return IdleOutcome::Timeout;
        }
        std::thread::sleep(left.min(Duration::from_millis(500)));
    }
}

fn store_flags(ops: &mut ImapOps, uid: u32, sign: char, flags: &[&str]) -> MailResult<()> {
    let (rt, session) = ops.parts()?;
    rt.block_on(async {
        let query = format!("{sign}FLAGS.SILENT ({})", flags.join(" "));
        let _: Vec<_> = session
            .uid_store(uid.to_string(), query)
            .await
            .map_err(proto)?
            .try_collect()
            .await
            .map_err(proto)?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PasswordSource;

    #[test]
    fn sleep_until_times_out_or_stops_on_interrupt() {
        let stop = AtomicBool::new(false);
        assert_eq!(
            sleep_until(Duration::from_millis(10), &stop),
            IdleOutcome::Timeout
        );
        stop.store(true, Ordering::Relaxed);
        assert_eq!(
            sleep_until(Duration::from_secs(60), &stop),
            IdleOutcome::Interrupted
        );
    }

    fn test_account(host: &str, port: u16) -> AccountConfig {
        AccountConfig {
            name: "test".into(),
            host: host.into(),
            port,
            username: "me@example.com".into(),
            password: PasswordSource::Keyring { keyring: true },
            address: None,
            aliases: vec![],
            sync_interval_secs: 120,
            trash_retention_days: 30,
            notify: true,
            ca_file: None,
        }
    }

    const TEST_CA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/dovecot/certs/ca.pem");

    #[test]
    fn root_store_adds_the_ca_file_to_the_public_roots() {
        let public = root_store(None).unwrap().len();
        assert_eq!(
            root_store(Some(Path::new(TEST_CA))).unwrap().len(),
            public + 1
        );
    }

    #[test]
    fn root_store_rejects_a_missing_or_empty_ca_file() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty.pem");
        std::fs::write(&empty, "not a certificate\n").unwrap();
        for path in [dir.path().join("missing.pem"), empty] {
            match root_store(Some(&path)) {
                Err(MailError::Connect(message)) => {
                    assert!(
                        message.contains("ca_file") && message.contains(&*path.to_string_lossy()),
                        "{message}"
                    )
                }
                other => panic!(
                    "{path:?}: expected a ca_file error, got {:?}",
                    other.map(|r| r.len())
                ),
            }
        }
    }

    #[test]
    fn connect_gives_up_on_a_server_that_never_answers() {
        // The kernel completes the TCP handshake from the backlog; nobody ever answers the TLS hello.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let started = Instant::now();
        let result = ImapOps::connect_within(
            &test_account("127.0.0.1", port),
            &Secret::new("x".into()),
            Duration::from_millis(300),
        );
        match result {
            Err(MailError::Connect(message)) => {
                assert!(message.contains("did not answer"), "{message}")
            }
            Err(other) => panic!("wrong error: {other}"),
            Ok(_) => panic!("connected to a silent server"),
        }
        assert!(started.elapsed() < Duration::from_secs(5));
        drop(listener);
    }

    #[test]
    fn keepalive_is_enabled_on_the_socket() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let tcp = TcpStream::connect(addr).await.unwrap();
            enable_keepalive(&tcp).unwrap();
            let socket = socket2::SockRef::from(&tcp);
            assert!(socket.keepalive().unwrap());
            assert_eq!(socket.tcp_keepalive_time().unwrap(), KEEPALIVE_IDLE);
        });
    }
}
