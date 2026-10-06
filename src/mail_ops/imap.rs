use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_imap::Session;
use async_imap::extensions::idle::IdleResponse;
use async_imap::types::{Fetch, Flag, NameAttribute};
use futures_util::TryStreamExt;
use tokio::net::TcpStream;
use tokio::runtime::Runtime;
use tokio_rustls::client::TlsStream;

use crate::config::AccountConfig;
use crate::credentials::Secret;
use crate::mail_ops::{
    Envelope, FlagUpdate, IdleOutcome, MailError, MailOps, MailResult, RemoteFolder, SelectInfo,
};

type ImapSession = Session<TlsStream<TcpStream>>;

pub struct ImapOps {
    rt: Runtime,
    session: Option<ImapSession>,
    has_move: bool,
    has_uidplus: bool,
}

impl ImapOps {
    pub fn connect(account: &AccountConfig, secret: &Secret) -> MailResult<ImapOps> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| MailError::Io(e.to_string()))?;
        let (session, has_move, has_uidplus) = rt.block_on(async {
            let tcp = TcpStream::connect((account.host.as_str(), account.port))
                .await
                .map_err(|e| MailError::Connect(e.to_string()))?;
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
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
            let has_move = caps.has_str("MOVE");
            let has_uidplus = caps.has_str("UIDPLUS");
            Ok::<_, MailError>((session, has_move, has_uidplus))
        })?;
        Ok(ImapOps {
            rt,
            session: Some(session),
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

fn envelope_from(fetch: &Fetch) -> Option<Envelope> {
    Some(Envelope {
        uid: fetch.uid?,
        flags: fetch.flags().map(|f| flag_to_string(&f)).collect(),
        internaldate: fetch.internal_date().map(|d| d.timestamp()).unwrap_or(0),
        size: fetch.size,
        headers: fetch.header().map(<[u8]>::to_vec).unwrap_or_default(),
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

    fn fetch_new(&mut self, from_uid: u32) -> MailResult<Vec<Envelope>> {
        let (rt, session) = self.parts()?;
        rt.block_on(async {
            let fetches: Vec<Fetch> = session
                .uid_fetch(
                    format!("{from_uid}:*"),
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
                .filter(|e| e.uid >= from_uid)
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

    fn append(&mut self, folder: &str, raw: &[u8]) -> MailResult<()> {
        let (rt, session) = self.parts()?;
        rt.block_on(async { session.append(folder, None, None, raw).await.map_err(proto) })
    }

    fn idle(&mut self, timeout: Duration, interrupt: &AtomicBool) -> MailResult<IdleOutcome> {
        let session = self
            .session
            .take()
            .ok_or_else(|| MailError::Io("session closed".into()))?;
        let (outcome, session) = self.rt.block_on(async {
            let mut handle = session.idle();
            handle.init().await.map_err(proto)?;
            let (wait, stop) = handle.wait_with_timeout(timeout.min(Duration::from_secs(29 * 60)));
            let mut stop = Some(stop);
            let outcome = {
                let mut wait = std::pin::pin!(wait);
                loop {
                    tokio::select! {
                        res = &mut wait => break match res.map_err(proto)? {
                            IdleResponse::NewData(_) => IdleOutcome::NewMail,
                            IdleResponse::Timeout => IdleOutcome::Timeout,
                            IdleResponse::ManualInterrupt => IdleOutcome::Interrupted,
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

    /// POSTBODE_TEST_IMAP_HOST, _USER, _PASS must be set. Lists folders and selects INBOX.
    #[test]
    #[ignore]
    fn live_round_trip() {
        let account = AccountConfig {
            name: "live".into(),
            host: std::env::var("POSTBODE_TEST_IMAP_HOST").unwrap(),
            port: 993,
            username: std::env::var("POSTBODE_TEST_IMAP_USER").unwrap(),
            password: PasswordSource::Keyring { keyring: true },
            address: Some("x@example.com".into()),
            aliases: vec![],
            sync_interval_secs: 120,
            trash_retention_days: 30,
            notify: true,
        };
        let secret = Secret::new(std::env::var("POSTBODE_TEST_IMAP_PASS").unwrap());
        let mut ops = ImapOps::connect(&account, &secret).unwrap();
        let folders = ops.list_folders().unwrap();
        assert!(folders.iter().any(|f| f.name.eq_ignore_ascii_case("INBOX")));
        let info = ops.select("INBOX").unwrap();
        assert!(info.uidvalidity > 0);
        let new = ops.fetch_new(1).unwrap();
        eprintln!(
            "{} messages, first headers {} bytes",
            new.len(),
            new.first().map(|e| e.headers.len()).unwrap_or(0)
        );
    }
}
