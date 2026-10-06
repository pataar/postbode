# GUI engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give front ends one library entry point (`postbode::engine`) that runs the account threads, accepts commands, reports live activity, and syncs large folders in resumable chunks. `postbode run` uses it; the egui app (plan 5) will too.

**Architecture:** Each account keeps one sync thread and one IMAP connection. Commands go into that thread over a channel; a per-account wake flag ends IDLE within 500 ms, and commands are also drained at checkpoints inside a pass (before each folder, between header chunks). Envelopes are fetched 500 messages at a time from a `UID SEARCH` list, each chunk committed on its own; migration 002 records which uids were already on the server so a resumed first sync stays silent. A lock file per account keeps two processes from syncing the same account.

**Tech Stack:** Rust 1.99 (edition 2024), async-imap 0.12 behind the sync `MailOps` trait, rusqlite + rusqlite_migration, std `File::try_lock`.

**Spec:** `docs/superpowers/specs/2026-10-06-postbode-gui-design.md` (§3 engine, §4 sync changes, §8 `set_enabled`, §10 tests). Core spec: `docs/superpowers/specs/2026-10-06-postbode-core-design.md`.

## Global Constraints

- No new runtime dependencies in this plan. `File::try_lock` is std (stable since 1.89).
- Every new dependency would need a one-line reason in `Cargo.toml`; there are none here.
- Migration 001 is live; schema changes go in `migrations/002-initial-uid-next/up.sql`.
- Nothing outside `src/mail_ops/imap.rs` is async.
- Definition of done per task: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test` pass. Live tests: `docker compose -f tests/dovecot/compose.yml up -d` then `POSTBODE_TEST_IMAP_HOST=localhost cargo test --test imap_live`.
- Never read message bodies from a user's store; never log bodies or secrets.
- After changing CLI doc comments, run `POSTBODE_BLESS=1 cargo test` and commit the regenerated `docs/src/cli.md`.
- Comments: default none; one line when the why is non-obvious; no history references.
- Conventional commits. Agents never push.
- Chunk size is 500 messages. IDLE wake latency is at most 500 ms (the existing poll in `ImapOps::idle`).

## Review Focus

1. A GUI command for a message a rule just moved away: the command reports an error for that uid in `ActionDone` and the session keeps running. Test in Task 4.
2. `Command::Restore` with a path outside the account's trash dir (the command type becomes a socket protocol later): refused, file untouched, nothing appended. Test in Task 4.
3. Quitting while an account is offline: `Engine::stop` returns within seconds, not after the 300 s backoff. Tests in Tasks 5 and 6.
4. `SyncNow` sent during a long first sync: drained at a checkpoint, then a second full pass runs; no lost command, no deadlock. Test in Task 5.
5. A command run between header chunks selects another folder: the chunk loop re-selects its folder before fetching again. Test in Task 3.
6. Commands sent while an account is offline: each one cuts the current reconnect wait once, never collapses the backoff into a tight loop. Covered by the `swap(false)` in Task 5's sleep closure and `stop_returns_promptly_while_an_account_is_offline` in Task 6.

---

### Task 1: Store — migration 002 and list summaries

**Files:**
- Create: `migrations/002-initial-uid-next/up.sql`
- Modify: `src/store.rs` (new methods after `upsert_folder`; new types after `Message`; tests at the end)

**Interfaces:**
- Produces:
  - `Store::initial_uid_next(&self, folder: &str) -> Result<u32, StoreError>` (0 when unset or no row)
  - `Store::set_initial_uid_next(&self, folder: &str, uid_next: u32) -> Result<(), StoreError>`
  - `pub struct MessageSummary { pub uid: u32, pub from: String, pub to: String, pub subject: String, pub date: i64, pub flags: String }` with `is_seen()` and `is_flagged()`
  - `pub struct ThreadSummary { pub thread_id: String, pub latest: MessageSummary, pub count: u32, pub unread: bool, pub flagged: bool }`
  - `Store::thread_summaries(&self, folder: &str, limit: u32) -> Result<Vec<ThreadSummary>, StoreError>` — most recently active thread first
  - `Store::thread_members(&self, folder: &str, thread_id: &str) -> Result<Vec<MessageSummary>, StoreError>` — oldest first

- [ ] **Step 1: Write the failing tests** (append inside `mod tests` in `src/store.rs`; `msg` and `store_with_inbox` already exist there)

```rust
    #[test]
    fn migration_002_keeps_rows_and_starts_at_zero() {
        let mut conn = Connection::open_in_memory().unwrap();
        Store::migrations().to_version(&mut conn, 1).unwrap();
        conn.execute(
            "INSERT INTO folders (name, uidvalidity, last_uid) VALUES ('INBOX', 7, 42)",
            [],
        )
        .unwrap();
        let s = Store::init(conn).unwrap();
        let inbox = s.folder("INBOX").unwrap().unwrap();
        assert_eq!((inbox.uidvalidity, inbox.last_uid), (7, 42));
        assert_eq!(s.initial_uid_next("INBOX").unwrap(), 0);
    }

    #[test]
    fn initial_uid_next_round_trips_and_survives_upsert() {
        let s = store_with_inbox();
        assert_eq!(s.initial_uid_next("INBOX").unwrap(), 0);
        assert_eq!(s.initial_uid_next("Nope").unwrap(), 0);
        s.set_initial_uid_next("INBOX", 1201).unwrap();
        s.upsert_folder(&Folder {
            name: "INBOX".into(),
            uidvalidity: 1,
            last_uid: 500,
            special_use: None,
        })
        .unwrap();
        assert_eq!(s.initial_uid_next("INBOX").unwrap(), 1201);
    }

    #[test]
    fn thread_summaries_describe_the_latest_message() {
        let s = store_with_inbox();
        let mut first = msg("INBOX", 1, 10);
        first.thread_id = "t1".into();
        first.flags = "\\Seen".into();
        let mut reply = msg("INBOX", 2, 30);
        reply.thread_id = "t1".into();
        reply.flags = "\\Flagged".into();
        let mut other = msg("INBOX", 3, 20);
        other.thread_id = "t2".into();
        other.flags = "\\Seen".into();
        for m in [&first, &reply, &other] {
            s.insert_message(m).unwrap();
        }
        let threads = s.thread_summaries("INBOX", 10).unwrap();
        assert_eq!(
            threads,
            vec![
                ThreadSummary {
                    thread_id: "t1".into(),
                    latest: MessageSummary {
                        uid: 2,
                        from: "Alice <alice@x>".into(),
                        to: "bob@x".into(),
                        subject: "subject 2".into(),
                        date: 30,
                        flags: "\\Flagged".into(),
                    },
                    count: 2,
                    unread: true,
                    flagged: true,
                },
                ThreadSummary {
                    thread_id: "t2".into(),
                    latest: MessageSummary {
                        uid: 3,
                        from: "Alice <alice@x>".into(),
                        to: "bob@x".into(),
                        subject: "subject 3".into(),
                        date: 20,
                        flags: "\\Seen".into(),
                    },
                    count: 1,
                    unread: false,
                    flagged: false,
                },
            ]
        );
        assert_eq!(s.thread_summaries("INBOX", 1).unwrap().len(), 1);
        let members: Vec<u32> = s
            .thread_members("INBOX", "t1")
            .unwrap()
            .iter()
            .map(|m| m.uid)
            .collect();
        assert_eq!(members, [1, 2]);
    }

    #[test]
    fn summaries_tolerate_missing_headers() {
        let s = store_with_inbox();
        let mut bare = msg("INBOX", 1, 10);
        bare.from_addr = None;
        bare.to_addr = None;
        bare.subject = None;
        s.insert_message(&bare).unwrap();
        let latest = &s.thread_summaries("INBOX", 10).unwrap()[0].latest;
        assert_eq!((latest.from.as_str(), latest.subject.as_str()), ("", ""));
        assert!(!latest.is_seen() && !latest.is_flagged());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib store::tests`
Expected: compile errors — `initial_uid_next`, `ThreadSummary`, `MessageSummary`, `thread_summaries` not found.

- [ ] **Step 3: Write the migration**

`migrations/002-initial-uid-next/up.sql`:

```sql
-- One above the highest uid already on the server when the folder was first tracked; lower uids never notify.
ALTER TABLE folders ADD COLUMN initial_uid_next INTEGER NOT NULL DEFAULT 0;
```

- [ ] **Step 4: Implement the store methods and types**

After the `impl Message` block:

```rust
/// What a list row shows; no headers or body.
#[derive(Debug, Clone, PartialEq)]
pub struct MessageSummary {
    pub uid: u32,
    pub from: String,
    pub to: String,
    pub subject: String,
    pub date: i64,
    pub flags: String,
}

impl MessageSummary {
    pub fn is_seen(&self) -> bool {
        self.flags.split(' ').any(|f| f == "\\Seen")
    }

    pub fn is_flagged(&self) -> bool {
        self.flags.split(' ').any(|f| f == "\\Flagged")
    }
}

/// A collapsed thread row: its latest message plus counts over all members in the folder.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadSummary {
    pub thread_id: String,
    pub latest: MessageSummary,
    pub count: u32,
    pub unread: bool,
    pub flagged: bool,
}

fn summary_from_row(row: &rusqlite::Row, offset: usize) -> rusqlite::Result<MessageSummary> {
    Ok(MessageSummary {
        uid: row.get(offset)?,
        from: row.get(offset + 1)?,
        to: row.get(offset + 2)?,
        subject: row.get(offset + 3)?,
        date: row.get(offset + 4)?,
        flags: row.get(offset + 5)?,
    })
}

const SUMMARY_COLUMNS: &str =
    "uid, COALESCE(from_addr, ''), COALESCE(to_addr, ''), COALESCE(subject, ''), internaldate, flags";
```

`Store::init` is private and called by the migration test, which lives in the same module, so no visibility change is needed. In `impl Store`, after `upsert_folder`:

```rust
    /// One above the highest uid already on the server when `folder` was first tracked; 0 when unknown.
    pub fn initial_uid_next(&self, folder: &str) -> Result<u32, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT initial_uid_next FROM folders WHERE name = ?1",
                params![folder],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0))
    }

    pub fn set_initial_uid_next(&self, folder: &str, uid_next: u32) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE folders SET initial_uid_next = ?2 WHERE name = ?1",
            params![folder, uid_next],
        )?;
        Ok(())
    }
```

After `threads`:

```rust
    /// One row per thread in `folder`, the most recently active first. With exactly one MAX() in the query, SQLite fills
    /// the bare columns from the row holding it: the thread's latest message. A second MAX() anywhere breaks that.
    pub fn thread_summaries(&self, folder: &str, limit: u32) -> Result<Vec<ThreadSummary>, StoreError> {
        let mut stmt = self.conn.prepare(&format!(
            r"SELECT thread_id, {SUMMARY_COLUMNS}, MAX(internaldate) AS latest, COUNT(*),
                     SUM(instr(' ' || flags || ' ', ' \Seen ') = 0), SUM(instr(' ' || flags || ' ', ' \Flagged ') > 0)
              FROM messages WHERE folder = ?1 GROUP BY thread_id
              ORDER BY latest DESC, uid DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![folder, limit], |r| {
            Ok(ThreadSummary {
                thread_id: r.get(0)?,
                latest: summary_from_row(r, 1)?,
                count: r.get(8)?,
                unread: r.get::<_, i64>(9)? > 0,
                flagged: r.get::<_, i64>(10)? > 0,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn thread_members(&self, folder: &str, thread_id: &str) -> Result<Vec<MessageSummary>, StoreError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {SUMMARY_COLUMNS} FROM messages WHERE folder = ?1 AND thread_id = ?2 ORDER BY internaldate, uid"
        ))?;
        let rows = stmt.query_map(params![folder, thread_id], |r| summary_from_row(r, 0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
```

Column indexes: 0 `thread_id`, 1–6 the summary columns, 7 `latest`, 8 `COUNT(*)`, 9 unread, 10 flagged. The query must keep exactly one `MAX()`/`MIN()` aggregate (SQLite's bare-column rule); `ORDER BY uid` uses the bare uid of the latest row.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib store::tests`
Expected: PASS, including the existing `migrations_are_valid`.

- [ ] **Step 6: Commit**

```bash
git add migrations/002-initial-uid-next/up.sql src/store.rs
git commit -m "feat(store): add initial_uid_next and thread summaries"
```

---

### Task 2: MailOps — list uids, fetch envelopes by range

**Files:**
- Modify: `src/mail_ops.rs` (trait, `RecordingOps`, its tests)
- Modify: `src/mail_ops/imap.rs` (replace `fetch_new`)
- Modify: `src/sync.rs:108-114` (the one caller, temporarily) and every test using `fail_fetch_new` or the `"fetch_new …"` call string

**Interfaces:**
- Produces (trait `MailOps`, replacing `fetch_new`):
  - `fn search_uids(&mut self, from_uid: u32) -> MailResult<Vec<u32>>` — uids `>= from_uid` in the selected folder, ascending
  - `fn fetch_envelopes(&mut self, first: u32, last: u32) -> MailResult<Vec<Envelope>>` — envelopes with `first <= uid <= last`
- `RecordingOps`: `fail_fetch_new: bool` becomes `pub fail_fetch_after: Option<usize>` (fail every `fetch_envelopes` call once that many have succeeded; `Some(0)` fails at once). Call strings: `"search_uids {folder} {from}"`, `"fetch_envelopes {folder} {first} {last}"`.

- [ ] **Step 1: Write the failing tests** (in `src/mail_ops.rs` `mod tests`, replacing `fake_fetch_new_mimics_star_semantics`)

```rust
    #[test]
    fn fake_search_uids_mimics_star_semantics() {
        let mut ops = RecordingOps::new().with_folder("INBOX", None);
        ops.add_mail("INBOX", 3, 10, "Subject: a\r\n\r\n", None);
        ops.add_mail("INBOX", 9, 20, "Subject: b\r\n\r\n", None);
        ops.select("INBOX").unwrap();
        assert_eq!(ops.search_uids(1).unwrap(), [3, 9]);
        assert_eq!(ops.search_uids(4).unwrap(), [9]);
        // Real servers answer `N:*` with the highest message when N exceeds it.
        assert_eq!(ops.search_uids(50).unwrap(), [9]);
        assert_eq!(ops.calls.last().unwrap(), "search_uids INBOX 50");
    }

    #[test]
    fn fake_fetch_envelopes_returns_the_range_and_can_fail_later() {
        let mut ops = RecordingOps::new().with_folder("INBOX", None);
        for uid in [1, 5, 9] {
            ops.add_mail("INBOX", uid, 10, "Subject: a\r\n\r\n", None);
        }
        ops.select("INBOX").unwrap();
        ops.fail_fetch_after = Some(1);
        let uids: Vec<u32> = ops.fetch_envelopes(2, 9).unwrap().iter().map(|e| e.uid).collect();
        assert_eq!(uids, [5, 9]);
        assert_eq!(ops.calls.last().unwrap(), "fetch_envelopes INBOX 2 9");
        assert!(matches!(ops.fetch_envelopes(1, 1), Err(MailError::Io(_))));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib mail_ops::tests`
Expected: compile errors — `search_uids`, `fetch_envelopes`, `fail_fetch_after` not found.

- [ ] **Step 3: Change the trait and the fake**

Trait, replacing `fetch_new`:

```rust
    /// Uids >= `from_uid` in the selected folder, ascending.
    fn search_uids(&mut self, from_uid: u32) -> MailResult<Vec<u32>>;
    /// Envelopes with `first <= uid <= last` in the selected folder.
    fn fetch_envelopes(&mut self, first: u32, last: u32) -> MailResult<Vec<Envelope>>;
```

`RecordingOps`: replace field `fail_fetch_new: bool` with

```rust
        /// Makes `fetch_envelopes` fail once this many calls succeeded, like a connection dropping mid-sync.
        pub fail_fetch_after: Option<usize>,
        envelope_fetches: usize,
```

(initialise both in `new()`: `None`, `0`), and replace the `fetch_new` impl with:

```rust
        fn search_uids(&mut self, from_uid: u32) -> MailResult<Vec<u32>> {
            self.calls
                .push(format!("search_uids {} {from_uid}", self.selected));
            let mut uids: Vec<u32> = self
                .mail
                .get(&self.selected)
                .map(|list| list.iter().map(|e| e.uid).collect())
                .unwrap_or_default();
            uids.sort_unstable();
            let max = uids.last().copied().unwrap_or(0);
            // Real servers answer `N:*` with the highest message when N exceeds it.
            if from_uid > max {
                return Ok(uids.into_iter().filter(|&uid| uid == max).collect());
            }
            Ok(uids.into_iter().filter(|&uid| uid >= from_uid).collect())
        }

        fn fetch_envelopes(&mut self, first: u32, last: u32) -> MailResult<Vec<Envelope>> {
            self.calls
                .push(format!("fetch_envelopes {} {first} {last}", self.selected));
            if self.fail_fetch_after.is_some_and(|n| self.envelope_fetches >= n) {
                return Err(MailError::Io("fetch_envelopes failed".into()));
            }
            self.envelope_fetches += 1;
            let mut out: Vec<Envelope> = self
                .mail
                .get(&self.selected)
                .map(|list| {
                    list.iter()
                        .filter(|e| (first..=last).contains(&e.uid))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            out.sort_by_key(|e| e.uid);
            Ok(out)
        }
```

- [ ] **Step 4: Implement on `ImapOps`** (replace `fn fetch_new` in `src/mail_ops/imap.rs`)

```rust
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
```

The `N:*` quirk is handled by the caller filtering `uid > last_uid` (Task 3 and Step 5 below).

- [ ] **Step 5: Keep `sync_folder` compiling** (Task 3 rewrites it; this is a bridge)

In `src/sync.rs`, replace

```rust
    let envelopes: Vec<Envelope> = ops
        .fetch_new(last_uid + 1)?
        .into_iter()
        .filter(|env| env.uid > last_uid)
        .collect();
```

with

```rust
    let uids: Vec<u32> = ops
        .search_uids(last_uid + 1)?
        .into_iter()
        .filter(|&uid| uid > last_uid)
        .collect();
    let envelopes: Vec<Envelope> = match (uids.first(), uids.last()) {
        (Some(&first), Some(&last)) => ops.fetch_envelopes(first, last)?,
        _ => Vec::new(),
    };
```

Update tests in `src/sync.rs`: `ops.fail_fetch_new = true` → `ops.fail_fetch_after = Some(0)`; `= false` → `= None`; the assertion `c == "fetch_new INBOX 3"` → `c == "search_uids INBOX 3"`. `ggrep -rn 'fail_fetch_new\|fetch_new' src tests` must print nothing afterwards.

An empty folder no longer fetches envelopes at all, so a failing fetch needs a message to fail on. In `backoff_grows_caps_and_resets_after_a_full_cycle`, the `_ =>` arm's ops get one: add `ops.add_mail("INBOX", 1, 0, "Subject: a\r\n\r\n", None);` after `with_folder`, and change its comment to `// The full pass skips the failing INBOX; after an IDLE wake the INBOX-only pass fails.` The expected sleep sequence stays `[5, 10, 20, 40, 80, 160, 300, 300, 300, 5]`.

- [ ] **Step 6: Run the whole suite**

Run: `cargo test`
Expected: PASS. Then with Dovecot up: `POSTBODE_TEST_IMAP_HOST=localhost cargo test --test imap_live` — PASS (11 tests).

- [ ] **Step 7: Commit**

```bash
git add src/mail_ops.rs src/mail_ops/imap.rs src/sync.rs
git commit -m "refactor(mail_ops): split fetch_new into search_uids and fetch_envelopes"
```

---

### Task 3: Chunked, resumable folder sync with checkpoints

**Files:**
- Modify: `src/sync.rs` (`sync_folder`, `sync_all`, `run_rules`; new `Activity`, `Checkpoint`, `SyncError::Stopped`; tests)

**Interfaces:**
- Consumes: Task 1 `initial_uid_next`/`set_initial_uid_next`; Task 2 `search_uids`/`fetch_envelopes`, `fail_fetch_after`.
- Produces:

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum Activity {
    Connecting,
    ListingFolders,
    SyncingFolder { folder: String, index: usize, of: usize },
    FetchingHeaders { folder: String, done: usize, total: usize },
    FetchingBodies { folder: String, done: usize, total: usize },
    RunningRules { folder: String },
    RunningCommand { what: String },
    Idle { since: i64 },
    Offline { reason: String, retry_at: i64 },
}

/// Called between steps of a pass with what is happening; may run queued commands on the connection.
/// Returns true when it used the connection, so the caller selects its folder again.
pub type Checkpoint<'a> = dyn FnMut(&mut dyn MailOps, Activity) -> Result<bool, SyncError> + 'a;

pub fn sync_folder_with(ops: &mut dyn MailOps, store: &Store, folder: &RemoteFolder, checkpoint: &mut Checkpoint<'_>) -> Result<Vec<NewMessageRef>, SyncError>;
pub fn sync_all_with(ops: &mut dyn MailOps, store: &Store, checkpoint: &mut Checkpoint<'_>) -> Result<(Vec<NewMessageRef>, Vec<String>), SyncError>;
pub fn run_rules_with(/* run_rules' nine parameters */, report: &mut dyn FnMut(Activity)) -> Result<RulesRun, SyncError>;
```

  `sync_folder`, `sync_all` and `run_rules` keep their signatures and call the `_with` versions with a no-op checkpoint (`&mut |_, _| Ok(false)`) or reporter (`&mut |_| {}`).
  `SyncError` gains `#[error("stopped")] Stopped`.

- [ ] **Step 1: Write the failing tests** (in `src/sync.rs` `mod tests`)

Replace the body of the existing `interrupted_first_sync_stays_initial_on_retry` from `ops.fail_fetch_after = Some(0);` down to the first `sync_folder` retry with:

```rust
        ops.fail_fetch_after = Some(0);
        let inbox = RemoteFolder {
            name: "INBOX".into(),
            special_use: None,
        };
        assert!(sync_folder(&mut ops, &store, &inbox).is_err());
        assert_eq!(store.folder("INBOX").unwrap().unwrap().last_uid, 0);
        assert_eq!(store.message_count("INBOX").unwrap(), 0);
        ops.fail_fetch_after = None;
        let new = sync_folder(&mut ops, &store, &inbox).unwrap();
        assert!(new.iter().all(|n| n.initial), "{new:?}");
```

(keep the rest of that test as is). Add:

```rust
    fn inbox() -> RemoteFolder {
        RemoteFolder {
            name: "INBOX".into(),
            special_use: None,
        }
    }

    #[test]
    fn sparse_uids_are_fetched_in_chunks_of_500_messages() {
        let mut ops = RecordingOps::new().with_folder("INBOX", None);
        for i in 1..=1200u32 {
            ops.add_mail("INBOX", i * 1000, 10 * H, &headers("a@x", "old", &format!("o{i}@x")), None);
        }
        let store = Store::open_in_memory().unwrap();
        let new = sync_folder(&mut ops, &store, &inbox()).unwrap();
        assert_eq!(new.len(), 1200);
        let fetches: Vec<&String> = ops
            .calls
            .iter()
            .filter(|c| c.starts_with("fetch_envelopes"))
            .collect();
        assert_eq!(
            fetches,
            [
                "fetch_envelopes INBOX 1000 500000",
                "fetch_envelopes INBOX 501000 1000000",
                "fetch_envelopes INBOX 1001000 1200000",
            ]
        );
        assert_eq!(store.folder("INBOX").unwrap().unwrap().last_uid, 1_200_000);
    }

    #[test]
    fn interrupted_chunked_first_sync_resumes_and_stays_silent() {
        let mut ops = RecordingOps::new().with_folder("INBOX", None);
        for uid in 1..=1200 {
            ops.add_mail("INBOX", uid, 10 * H, &headers("a@x", "old", &format!("o{uid}@x")), None);
        }
        let store = Store::open_in_memory().unwrap();
        ops.fail_fetch_after = Some(1);
        assert!(sync_folder(&mut ops, &store, &inbox()).is_err());
        assert_eq!(store.folder("INBOX").unwrap().unwrap().last_uid, 500);
        assert_eq!(store.message_count("INBOX").unwrap(), 500);
        assert_eq!(store.initial_uid_next("INBOX").unwrap(), 1201);

        ops.fail_fetch_after = None;
        ops.add_mail("INBOX", 1201, 11 * H, &headers("b@x", "new", "n@x"), None);
        let new = sync_folder(&mut ops, &store, &inbox()).unwrap();
        assert_eq!(new.len(), 701);
        assert!(new.iter().filter(|n| n.uid <= 1200).all(|n| n.initial));
        assert!(!new.iter().find(|n| n.uid == 1201).unwrap().initial);
    }

    #[test]
    fn a_folder_that_starts_empty_notifies_its_first_mail() {
        let mut ops = RecordingOps::new().with_folder("INBOX", None);
        let store = Store::open_in_memory().unwrap();
        assert!(sync_folder(&mut ops, &store, &inbox()).unwrap().is_empty());
        assert_eq!(store.initial_uid_next("INBOX").unwrap(), 0);
        ops.add_mail("INBOX", 1, 10 * H, &headers("a@x", "first", "f@x"), None);
        let new = sync_folder(&mut ops, &store, &inbox()).unwrap();
        assert!(!new[0].initial);
    }

    #[test]
    fn checkpoints_report_progress_in_order() {
        let mut ops = ops_with_inbox();
        let store = Store::open_in_memory().unwrap();
        let mut seen = Vec::new();
        sync_all_with(&mut ops, &store, &mut |_, activity| {
            seen.push(activity);
            Ok(false)
        })
        .unwrap();
        let folder = |name: &str, index| Activity::SyncingFolder {
            folder: name.into(),
            index,
            of: 2,
        };
        let headers = |done| Activity::FetchingHeaders {
            folder: "INBOX".into(),
            done,
            total: 2,
        };
        assert_eq!(
            seen,
            [
                Activity::ListingFolders,
                folder("INBOX", 1),
                headers(0),
                headers(2),
                folder("Trash", 2),
            ]
        );
    }

    #[test]
    fn a_checkpoint_that_used_the_connection_makes_the_chunk_loop_reselect() {
        let mut ops = ops_with_inbox();
        let store = Store::open_in_memory().unwrap();
        sync_folder_with(&mut ops, &store, &inbox(), &mut |ops, activity| {
            if matches!(activity, Activity::FetchingHeaders { done: 0, .. }) {
                ops.select("Trash")?;
                return Ok(true);
            }
            Ok(false)
        })
        .unwrap();
        let tail: Vec<&String> = ops.calls.iter().rev().take(3).rev().collect();
        assert_eq!(tail, ["select Trash", "select INBOX", "fetch_envelopes INBOX 1 2"]);
        assert_eq!(store.message_count("INBOX").unwrap(), 2);
    }

    #[test]
    fn a_stopping_checkpoint_ends_the_pass() {
        let mut ops = ops_with_inbox();
        let store = Store::open_in_memory().unwrap();
        let result = sync_all_with(&mut ops, &store, &mut |_, _| Err(SyncError::Stopped));
        assert!(matches!(result, Err(SyncError::Stopped)));
    }

    #[test]
    fn rules_report_running_and_body_progress() {
        let mut ops = ops_with_inbox();
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory().unwrap();
        let trash = Trash::new(dir.path().to_path_buf());
        let acc = account();
        let identity = acc.identity().unwrap();
        sync_all(&mut ops, &store).unwrap();
        ops.add_mail(
            "INBOX",
            3,
            12 * H,
            &headers("bob@x", "code", "m3@x"),
            Some("From: bob@x\r\n\r\nyour code 99"),
        );
        let new = sync_all(&mut ops, &store).unwrap().0;
        let rules = rules_from(
            "[[rules]]\nname = \"codes\"\nmatch.body = { contains = \"code\" }\nactions = [\"flag\"]\n",
            &store,
            0,
        );
        let mut seen = Vec::new();
        run_rules_with(
            &mut ops, &store, &trash, &rules, &acc, &identity, &new, Mode::Normal, 13 * H,
            &mut |a| seen.push(a),
        )
        .unwrap();
        assert!(seen.contains(&Activity::RunningRules { folder: "INBOX".into() }));
        assert!(seen.contains(&Activity::FetchingBodies {
            folder: "INBOX".into(),
            done: 1,
            total: 1
        }));
    }
```

If the rules file syntax for a body condition differs, copy the body rule used by an existing test in this module (search `body` in `mod tests`).

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib sync::tests`
Expected: compile errors — `Activity`, `sync_all_with`, `sync_folder_with`, `run_rules_with`, `SyncError::Stopped` not found.

- [ ] **Step 3: Implement**

Add `Activity`, `Checkpoint` (Interfaces above) after `RulesRun`, and `#[error("stopped")] Stopped,` to `SyncError`. Add `const CHUNK: usize = 500;` with the comment `/// Envelopes are fetched this many messages at a time, each batch committed on its own.`

Replace `sync_folder` with:

```rust
pub fn sync_folder(
    ops: &mut dyn MailOps,
    store: &Store,
    folder: &RemoteFolder,
) -> Result<Vec<NewMessageRef>, SyncError> {
    sync_folder_with(ops, store, folder, &mut |_, _| Ok(false))
}

pub fn sync_folder_with(
    ops: &mut dyn MailOps,
    store: &Store,
    folder: &RemoteFolder,
    checkpoint: &mut Checkpoint<'_>,
) -> Result<Vec<NewMessageRef>, SyncError> {
    let info = ops.select(&folder.name)?;
    let stored = store.folder(&folder.name)?;
    // Newly tracked, a placeholder, or reset: everything listed now was already on the server.
    let (last_uid, starts_tracking) = match &stored {
        Some(f) if f.uidvalidity == info.uidvalidity => (f.last_uid, false),
        // uidvalidity 0 marks a placeholder created by a rule move into a folder not synced yet.
        Some(f) if f.uidvalidity != 0 => {
            log::info!(
                "{}: UIDVALIDITY changed {} -> {}, resyncing",
                folder.name,
                f.uidvalidity,
                info.uidvalidity
            );
            (0, true)
        }
        _ => (0, true),
    };

    let updates = if last_uid > 0 {
        ops.fetch_flags(last_uid)?
    } else {
        Vec::new()
    };
    let uids: Vec<u32> = ops
        .search_uids(last_uid + 1)?
        .into_iter()
        .filter(|&uid| uid > last_uid)
        .collect();
    let row = |last_uid| Folder {
        name: folder.name.clone(),
        uidvalidity: info.uidvalidity,
        last_uid,
        special_use: folder.special_use.clone(),
    };

    store.transaction(|| {
        if starts_tracking && stored.is_some() {
            store.reset_folder(&folder.name, info.uidvalidity)?;
        }
        store.upsert_folder(&row(last_uid))?;
        if starts_tracking {
            store.set_initial_uid_next(&folder.name, uids.last().map_or(0, |uid| uid + 1))?;
        }
        let present: Vec<u32> = updates.iter().map(|u| u.uid).collect();
        for u in &updates {
            store.update_flags(&folder.name, u.uid, &u.flags.join(" "))?;
        }
        if last_uid > 0 {
            store.remove_missing(&folder.name, last_uid, &present)?;
        }
        Ok::<_, SyncError>(())
    })?;

    let initial_uid_next = store.initial_uid_next(&folder.name)?;
    let total = uids.len();
    let mut done = 0;
    let mut new = Vec::new();
    for chunk in uids.chunks(CHUNK) {
        let progress = Activity::FetchingHeaders {
            folder: folder.name.clone(),
            done,
            total,
        };
        // A command ran on the connection and may have selected another folder.
        if checkpoint(ops, progress)? && ops.select(&folder.name)?.uidvalidity != info.uidvalidity {
            break;
        }
        let (first, last) = (chunk[0], chunk[chunk.len() - 1]);
        let envelopes = ops.fetch_envelopes(first, last)?;
        store.transaction(|| {
            for env in envelopes {
                let uid = env.uid;
                store.insert_message(&to_message(&folder.name, env))?;
                new.push(NewMessageRef {
                    folder: folder.name.clone(),
                    uid,
                    initial: uid < initial_uid_next,
                });
            }
            store.upsert_folder(&row(last))
        })?;
        done += chunk.len();
    }
    if total > 0 {
        checkpoint(
            ops,
            Activity::FetchingHeaders {
                folder: folder.name.clone(),
                done,
                total,
            },
        )?;
    }
    Ok(new)
}
```

Update the `NewMessageRef::initial` doc comment to: `/// The uid was already on the server when the folder was first tracked or reset; such messages never notify.`

Replace `sync_all` with a wrapper plus:

```rust
pub fn sync_all(
    ops: &mut dyn MailOps,
    store: &Store,
) -> Result<(Vec<NewMessageRef>, Vec<String>), SyncError> {
    sync_all_with(ops, store, &mut |_, _| Ok(false))
}

/// Syncs every listed folder. A folder that fails is skipped and reported in the second list; the others still sync.
pub fn sync_all_with(
    ops: &mut dyn MailOps,
    store: &Store,
    checkpoint: &mut Checkpoint<'_>,
) -> Result<(Vec<NewMessageRef>, Vec<String>), SyncError> {
    checkpoint(ops, Activity::ListingFolders)?;
    let mut folders = ops.list_folders()?;
    special_use_by_name(&mut folders);
    let of = folders.len();
    let mut new = Vec::new();
    let mut errors = Vec::new();
    for (index, folder) in folders.iter().enumerate() {
        let syncing = Activity::SyncingFolder {
            folder: folder.name.clone(),
            index: index + 1,
            of,
        };
        checkpoint(ops, syncing)?;
        match sync_folder_with(ops, store, folder, checkpoint) {
            Ok(found) => new.extend(found),
            Err(SyncError::Stopped) => return Err(SyncError::Stopped),
            Err(e) => errors.push(format!("{}: {e}; skipped this pass", folder.name)),
        }
    }
    Ok((new, errors))
}
```

Rename `run_rules` to `run_rules_with`, add the parameter `report: &mut dyn FnMut(Activity)` last, and add a `run_rules` wrapper with the old nine parameters that passes `&mut |_| {}`. Inside the per-folder loop, after the UIDVALIDITY check and before iterating messages:

```rust
        report(Activity::RunningRules {
            folder: folder.clone(),
        });
        let messages = store.messages_in_folder(&folder)?;
        let bodies_total = if needs_body {
            messages
                .iter()
                .filter(|m| m.body_text.is_none() && fresh.contains(&(m.folder.as_str(), m.uid)))
                .count()
        } else {
            0
        };
        let mut bodies_done = 0;
```

iterate `for mut msg in messages` instead of calling `messages_in_folder` again, and after each `ensure_raw` attempt (success or failure) inside the body-fetch branch:

```rust
                bodies_done += 1;
                report(Activity::FetchingBodies {
                    folder: folder.clone(),
                    done: bodies_done,
                    total: bodies_total,
                });
```

`needs_body` is computed before this block today; move its `let` above `report(Activity::RunningRules …)` if needed.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib sync::tests`
Expected: PASS, including all earlier sync tests unchanged except the edited `interrupted_first_sync_stays_initial_on_retry`.

- [ ] **Step 5: Run the whole suite and live tests**

Run: `cargo test && POSTBODE_TEST_IMAP_HOST=localhost cargo test --test imap_live`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/sync.rs
git commit -m "feat(sync): fetch envelopes in resumable 500-message chunks with progress checkpoints"
```

---

### Task 4: Commands, new events, and trash restore in the library

**Files:**
- Modify: `src/trash.rs` (add `RestoreError`, `Trash::restore`, tests)
- Modify: `src/actions.rs` (add `fetch_body`)
- Modify: `src/sync.rs` (add `Command`, `CommandsRun`, `run_commands`, new `Event` variants, `SyncError` variants; tests)
- Modify: `src/cli/mod.rs` (`TrashCommand::Restore` calls `Trash::restore`; `print_event` covers the new variants)

**Interfaces:**
- Consumes: Task 3 `Activity`.
- Produces:

```rust
// trash.rs
#[derive(Debug, thiserror::Error)]
pub enum RestoreError {
    #[error("{0} is not a backup in this account's trash")]
    NotInTrash(String),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Mail(#[from] MailError),
}
impl Trash { pub fn restore(&self, ops: &mut dyn MailOps, file: &Path) -> Result<String, RestoreError>; } // returns the folder

// actions.rs
pub fn fetch_body(ops: &mut dyn MailOps, store: &Store, folder: &str, uid: u32) -> Result<(), ActionError>;

// sync.rs
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Apply { folder: String, uids: Vec<u32>, action: Action },
    FetchBody { folder: String, uid: u32 },
    Restore { file: PathBuf },
    SyncNow,
}
#[derive(Debug, Default, PartialEq)]
pub struct CommandsRun { pub used_connection: bool, pub wants_full_pass: bool }
pub fn run_commands(ops: &mut dyn MailOps, store: &Store, trash: &Trash, account: &AccountConfig, commands: &Receiver<Command>, events: &Sender<Event>) -> Result<CommandsRun, SyncError>;
// Event gains:
//   BodyReady { account: String, folder: String, uid: u32 },
//   ActionDone { account: String, folder: String, results: Vec<(u32, Result<usize, String>)> },
//   Restored { account: String, folder: String },
//   Activity { account: String, activity: Activity },
// SyncError gains: Action(#[from] ActionError), Restore(#[from] RestoreError)
```

- [ ] **Step 1: Write the failing tests**

In `src/sync.rs` `mod tests`:

```rust
    fn synced() -> (RecordingOps, Store, tempfile::TempDir) {
        let mut ops = ops_with_inbox();
        let store = Store::open_in_memory().unwrap();
        sync_all(&mut ops, &store).unwrap();
        (ops, store, tempfile::tempdir().unwrap())
    }

    fn drain(
        ops: &mut RecordingOps,
        store: &Store,
        trash: &Trash,
        commands: Vec<Command>,
    ) -> (Result<CommandsRun, SyncError>, Vec<Event>) {
        let (tx, rx) = std::sync::mpsc::channel();
        for c in commands {
            tx.send(c).unwrap();
        }
        let (events, seen) = std::sync::mpsc::channel();
        let run = run_commands(ops, store, trash, &account(), &rx, &events);
        (run, seen.try_iter().collect())
    }

    #[test]
    fn apply_reports_a_result_per_uid_and_keeps_going() {
        let (mut ops, store, dir) = synced();
        let trash = Trash::new(dir.path().to_path_buf());
        let apply = Command::Apply {
            folder: "INBOX".into(),
            uids: vec![1, 99],
            action: Action::MarkRead,
        };
        let (run, events) = drain(&mut ops, &store, &trash, vec![apply, Command::SyncNow]);
        assert_eq!(
            run.unwrap(),
            CommandsRun {
                used_connection: true,
                wants_full_pass: true
            }
        );
        let done = events
            .iter()
            .find_map(|e| match e {
                Event::ActionDone { results, .. } => Some(results.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(done[0], (1, Ok(1)));
        assert!(matches!(&done[1], (99, Err(e)) if e.contains("no message INBOX/99")), "{done:?}");
        assert!(store.message("INBOX", 1).unwrap().unwrap().is_seen());
        assert!(events.contains(&Event::Activity {
            account: "work".into(),
            activity: Activity::RunningCommand {
                what: "marking read 2 messages".into()
            }
        }));
    }

    #[test]
    fn fetch_body_stores_the_body_and_reports_it() {
        let (mut ops, store, dir) = synced();
        let trash = Trash::new(dir.path().to_path_buf());
        let (run, events) = drain(
            &mut ops,
            &store,
            &trash,
            vec![Command::FetchBody {
                folder: "INBOX".into(),
                uid: 1,
            }],
        );
        assert!(run.unwrap().used_connection);
        assert!(store.raw("INBOX", 1).unwrap().is_some());
        assert!(events.contains(&Event::BodyReady {
            account: "work".into(),
            folder: "INBOX".into(),
            uid: 1
        }));
    }

    #[test]
    fn restore_appends_the_backup_and_asks_for_a_full_pass() {
        let (mut ops, store, dir) = synced();
        let trash = Trash::new(dir.path().to_path_buf());
        let file = trash.save("INBOX", 7, b"Subject: back\r\n\r\nhi", 100).unwrap();
        let (run, events) = drain(&mut ops, &store, &trash, vec![Command::Restore { file: file.clone() }]);
        assert!(run.unwrap().wants_full_pass);
        assert!(!file.exists());
        assert!(ops.calls.iter().any(|c| c.starts_with("append INBOX")));
        assert!(events.contains(&Event::Restored {
            account: "work".into(),
            folder: "INBOX".into()
        }));
    }

    #[test]
    fn restore_refuses_a_file_outside_the_trash() {
        let (mut ops, store, dir) = synced();
        let trash = Trash::new(dir.path().join("trash"));
        let outside = dir.path().join("100-INBOX-7.eml");
        std::fs::write(&outside, b"Subject: x\r\n\r\n").unwrap();
        let (run, events) = drain(&mut ops, &store, &trash, vec![Command::Restore { file: outside.clone() }]);
        assert!(run.is_ok());
        assert!(outside.exists());
        assert!(!ops.calls.iter().any(|c| c.starts_with("append")));
        assert!(events.iter().any(|e| matches!(e, Event::Error { message, .. } if message.contains("not a backup"))));
    }

    #[test]
    fn a_command_for_a_vanished_message_reports_and_the_next_runs() {
        let (mut ops, store, dir) = synced();
        let trash = Trash::new(dir.path().to_path_buf());
        store.remove_message("INBOX", 1).unwrap();
        let commands = vec![
            Command::FetchBody {
                folder: "INBOX".into(),
                uid: 1,
            },
            Command::FetchBody {
                folder: "INBOX".into(),
                uid: 2,
            },
        ];
        let (run, events) = drain(&mut ops, &store, &trash, commands);
        assert!(run.is_ok());
        assert!(events.iter().any(|e| matches!(e, Event::Error { message, .. } if message.contains("INBOX/1"))));
        assert!(events.iter().any(|e| matches!(e, Event::BodyReady { uid: 2, .. })));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib sync::tests`
Expected: compile errors — `Command`, `CommandsRun`, `run_commands`, new `Event` variants not found.

- [ ] **Step 3: Move restore into `trash.rs`**

```rust
use std::path::Path;

use crate::mail_ops::{MailError, MailOps};
use crate::rules::engine::RESTORED_KEYWORD;
```

(add `RestoreError` from Interfaces), and in `impl Trash`:

```rust
    /// Appends the backup to its original folder with the restored keyword, then removes the file. Returns the folder.
    pub fn restore(&self, ops: &mut dyn MailOps, file: &Path) -> Result<String, RestoreError> {
        let not_in_trash = || RestoreError::NotInTrash(file.display().to_string());
        let file = file.canonicalize().map_err(|_| not_in_trash())?;
        let in_trash = self.dir.canonicalize().ok().as_deref() == file.parent();
        let parsed = file
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(Trash::parse_name);
        let (Some((_, folder, _)), true) = (parsed, in_trash) else {
            return Err(not_in_trash());
        };
        let raw = std::fs::read(&file)?;
        ops.append(&folder, &raw, &[RESTORED_KEYWORD])?;
        std::fs::remove_file(&file)?;
        Ok(folder)
    }
```

In `src/cli/mod.rs`, `TrashCommand::Restore` becomes:

```rust
        TrashCommand::Restore { file, account } => {
            let acc = single_account(config, account.as_deref())?;
            let mut ops = sync::connect(acc)?;
            let folder = Trash::new(paths.trash_dir(&acc.name)).restore(&mut ops, Path::new(&file))?;
            println!(
                "restored to {}; rules leave restored mail alone. Run `postbode sync` to see it",
                clean(&folder, false)
            );
            Ok(())
        }
```

- [ ] **Step 4: Add `actions::fetch_body`**

```rust
/// Downloads and indexes one message body, refusing when the folder changed on the server since the last sync.
pub fn fetch_body(
    ops: &mut dyn MailOps,
    store: &Store,
    folder: &str,
    uid: u32,
) -> Result<(), ActionError> {
    select_synced(ops, store, folder)?;
    let msg = store.message(folder, uid)?.ok_or_else(|| ActionError::NotFound {
        folder: folder.to_string(),
        uid,
    })?;
    ensure_raw(&msg, ops, store)?;
    Ok(())
}
```

- [ ] **Step 5: Add commands and events to `sync.rs`**

Add the `Command`, `CommandsRun` types and `Event` variants from Interfaces (`use crate::rules::Action;`, `use crate::actions::{self, ActionError};`, `use crate::trash::RestoreError;`, `use std::sync::mpsc::Receiver;`), the two `SyncError` variants (`#[error(transparent)]`), and:

```rust
/// Runs every queued command in arrival order. A failed command is reported and the next one runs; a lost
/// connection ends the drain with the error, so the session reconnects.
pub fn run_commands(
    ops: &mut dyn MailOps,
    store: &Store,
    trash: &Trash,
    account: &AccountConfig,
    commands: &Receiver<Command>,
    events: &Sender<Event>,
) -> Result<CommandsRun, SyncError> {
    let name = || account.name.clone();
    let mut run = CommandsRun::default();
    while let Ok(command) = commands.try_recv() {
        match command {
            Command::SyncNow => run.wants_full_pass = true,
            Command::Apply { folder, uids, action } => {
                run.used_connection = true;
                let what = describe(&action, uids.len());
                let _ = events.send(Event::Activity {
                    account: name(),
                    activity: Activity::RunningCommand { what },
                });
                let (results, lost) = match actions::run(ops, store, trash, &folder, &uids, &action, now()) {
                    Ok(results) => (
                        results.into_iter().map(|(uid, r)| (uid, r.map_err(|e| e.to_string()))).collect(),
                        None,
                    ),
                    Err(e) => (
                        uids.iter().map(|&uid| (uid, Err(e.to_string()))).collect(),
                        connection_lost(&e).then_some(e),
                    ),
                };
                let _ = events.send(Event::ActionDone {
                    account: name(),
                    folder,
                    results,
                });
                if let Some(e) = lost {
                    return Err(e.into());
                }
            }
            Command::FetchBody { folder, uid } => {
                run.used_connection = true;
                match actions::fetch_body(ops, store, &folder, uid) {
                    Ok(()) => {
                        let _ = events.send(Event::BodyReady {
                            account: name(),
                            folder,
                            uid,
                        });
                    }
                    Err(e) if connection_lost(&e) => return Err(e.into()),
                    Err(e) => {
                        let _ = events.send(account_error(account, format!("{folder}/{uid}: {e}")));
                    }
                }
            }
            Command::Restore { file } => {
                run.used_connection = true;
                match trash.restore(ops, &file) {
                    Ok(folder) => {
                        run.wants_full_pass = true;
                        let _ = events.send(Event::Restored {
                            account: name(),
                            folder,
                        });
                    }
                    Err(RestoreError::Mail(e @ (MailError::Io(_) | MailError::Connect(_)))) => {
                        return Err(SyncError::Mail(e));
                    }
                    Err(e) => {
                        let _ = events.send(account_error(account, e.to_string()));
                    }
                }
            }
        }
    }
    Ok(run)
}

fn connection_lost(e: &ActionError) -> bool {
    matches!(e, ActionError::Mail(MailError::Io(_) | MailError::Connect(_)))
}

fn describe(action: &Action, count: usize) -> String {
    let verb = match action {
        Action::Archive => "archiving",
        Action::Delete | Action::Trash => "deleting",
        Action::Flag => "flagging",
        Action::MarkRead => "marking read",
        Action::MarkUnread => "marking unread",
        Action::Move(_) => "moving",
        Action::Notify | Action::Silent => "updating",
        Action::Unflag => "unflagging",
    };
    let plural = if count == 1 { "" } else { "s" };
    format!("{verb} {count} message{plural}")
}
```

- [ ] **Step 6: Keep the CLI printing exhaustive**

In `print_event` (`src/cli/mod.rs`), add before the closing brace of the `match`:

```rust
        Event::Activity { account, activity } => log::debug!("[{account}] {activity:?}"),
        Event::ActionDone { .. } | Event::BodyReady { .. } | Event::Restored { .. } => {}
```

- [ ] **Step 7: Run tests**

Run: `cargo test && POSTBODE_TEST_IMAP_HOST=localhost cargo test --test imap_live`
Expected: PASS (the live `rule_delete_keeps_a_backup_then_expunges` and any trash-restore test still pass through the new `Trash::restore`).

- [ ] **Step 8: Commit**

```bash
git add src/trash.rs src/actions.rs src/sync.rs src/cli/mod.rs
git commit -m "feat(sync): add commands, command results and activity events"
```

---

### Task 5: The run loop takes commands and a wake flag

**Files:**
- Modify: `src/sync.rs` (`run_loop`, `run_loop_with`, `run_session`, `AccountSync::pass`, `run_once`; tests)
- Modify: `src/mail_ops.rs` (`RecordingOps::idle` semantics, new field)

**Interfaces:**
- Consumes: Task 3 `sync_all_with`/`sync_folder_with`/`run_rules_with`, Task 4 `run_commands`, `Command`, `Event::Activity`.
- Produces:

```rust
pub fn run_loop(account: AccountConfig, paths: Paths, events: Sender<Event>, shutdown: Arc<AtomicBool>, commands: Receiver<Command>, wake: Arc<AtomicBool>);
pub fn run_loop_with(account: AccountConfig, paths: Paths, events: Sender<Event>, shutdown: Arc<AtomicBool>, commands: Receiver<Command>, wake: Arc<AtomicBool>, connect: impl FnMut() -> Result<Box<dyn MailOps>, SyncError>, sleep: impl FnMut(Duration));
```

  Semantics: `wake` is the `interrupt` passed to `MailOps::idle`. Whoever stops the loop sets `shutdown` and then `wake`. `IdleOutcome::Interrupted` with `shutdown` unset means "commands are waiting": clear `wake`, drain, return to IDLE without a pass unless a command asked for one.
- `RecordingOps` (test fake): `pub shutdown_when_idle_empty: Option<Arc<AtomicBool>>`. When `idle_outcomes` is empty: if `interrupt` is set return `Interrupted`; else if the field is `Some(flag)`, set `flag` and return `Interrupted`; else wait in 10 ms steps until `interrupt` is set, then return `Interrupted`.

- [ ] **Step 1: Update the fake's idle**

In `RecordingOps` add the field (init `None`) and replace `fn idle` with:

```rust
        fn idle(&mut self, _timeout: Duration, interrupt: &AtomicBool) -> MailResult<IdleOutcome> {
            self.calls.push(format!("idle {}", self.selected));
            if interrupt.load(Ordering::Relaxed) {
                return Ok(IdleOutcome::Interrupted);
            }
            if let Some(outcome) = self.idle_outcomes.pop_front() {
                return Ok(outcome);
            }
            if let Some(shutdown) = &self.shutdown_when_idle_empty {
                shutdown.store(true, Ordering::Relaxed);
                return Ok(IdleOutcome::Interrupted);
            }
            // Like a real server: wait until someone wakes the session.
            while !interrupt.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(IdleOutcome::Interrupted)
        }
```

(`use std::sync::Arc;` in the `recording` module.)

- [ ] **Step 2: Write the failing tests** (in `src/sync.rs` `mod tests`)

First update every existing `run_loop_with(...)` call: add `commands` and `wake` arguments after `shutdown` (`std::sync::mpsc::channel::<Command>().1` and `Arc::new(AtomicBool::new(false))` where the test does not use them), and give each `RecordingOps` that reaches IDLE `ops.shutdown_when_idle_empty = Some(shutdown.clone());` (create the `shutdown` Arc before the ops where needed). Then add:

```rust
    /// Receives until `wanted` matches, keeping every event in `log`.
    fn wait_for(
        rx: &std::sync::mpsc::Receiver<Event>,
        log: &mut Vec<Event>,
        wanted: impl Fn(&Event) -> bool,
    ) -> Event {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let event = rx
                .recv_timeout(left)
                .unwrap_or_else(|e| panic!("no matching event within 5 s: {e}; got {log:?}"));
            log.push(event.clone());
            if wanted(&event) {
                return event;
            }
        }
    }

    #[test]
    fn a_command_sent_during_idle_runs_on_the_same_session() {
        let ops = ops_with_inbox();
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let (tx, rx) = std::sync::mpsc::channel();
        let (commands_tx, commands) = std::sync::mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(AtomicBool::new(false));
        let worker = {
            let (shutdown, wake) = (shutdown.clone(), wake.clone());
            let mut ops = Some(ops);
            std::thread::spawn(move || {
                let mut connects = 0;
                run_loop_with(
                    account(),
                    paths,
                    tx,
                    shutdown,
                    commands,
                    wake,
                    || {
                        connects += 1;
                        Ok(Box::new(ops.take().expect("one connection")) as Box<dyn MailOps>)
                    },
                    |_| panic!("the session must not fail"),
                );
                connects
            })
        };
        let mut log = Vec::new();
        wait_for(&rx, &mut log, |e| matches!(e, Event::Activity { activity: Activity::Idle { .. }, .. }));
        commands_tx
            .send(Command::Apply {
                folder: "INBOX".into(),
                uids: vec![1],
                action: Action::MarkRead,
            })
            .unwrap();
        wake.store(true, Ordering::Relaxed);
        let done = wait_for(&rx, &mut log, |e| matches!(e, Event::ActionDone { .. }));
        assert!(matches!(done, Event::ActionDone { results, .. } if results == [(1, Ok(1))]));
        wait_for(&rx, &mut log, |e| matches!(e, Event::Activity { activity: Activity::Idle { .. }, .. }));
        shutdown.store(true, Ordering::Relaxed);
        wake.store(true, Ordering::Relaxed);
        assert_eq!(worker.join().unwrap(), 1);
        log.extend(rx.try_iter());
        let passes = log.iter().filter(|e| matches!(e, Event::Synced { .. })).count();
        assert_eq!(passes, 1, "only the first pass; the command ran without one: {log:?}");
    }

    #[test]
    fn sync_now_during_a_pass_runs_another_full_pass() {
        let mut ops = ops_with_inbox();
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let (tx, rx) = std::sync::mpsc::channel();
        let (commands_tx, commands) = std::sync::mpsc::channel();
        commands_tx.send(Command::SyncNow).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        ops.shutdown_when_idle_empty = Some(shutdown.clone());
        run_loop_with(
            account(),
            paths,
            tx,
            shutdown,
            commands,
            Arc::new(AtomicBool::new(false)),
            || Ok(Box::new(std::mem::take(&mut ops)) as Box<dyn MailOps>),
            |_| panic!("the session must not fail"),
        );
        let events: Vec<Event> = rx.try_iter().collect();
        let passes = events.iter().filter(|e| matches!(e, Event::Synced { .. })).count();
        let listings = events
            .iter()
            .filter(|e| matches!(e, Event::Activity { activity: Activity::ListingFolders, .. }))
            .count();
        assert_eq!((passes, listings), (2, 2), "{events:?}");
    }

    #[test]
    fn stop_interrupts_the_reconnect_wait() {
        let mut offline = account();
        offline.host = "127.0.0.1".into();
        offline.port = 1;
        offline.password = PasswordSource::Command {
            command: "printf x".into(),
        };
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(AtomicBool::new(false));
        let worker = {
            let (paths, shutdown, wake) = (Paths::under(dir.path()), shutdown.clone(), wake.clone());
            let commands = std::sync::mpsc::channel::<Command>().1;
            std::thread::spawn(move || run_loop(offline, paths, tx, shutdown, commands, wake))
        };
        wait_for(&rx, &mut Vec::new(), |e| matches!(e, Event::Activity { activity: Activity::Offline { .. }, .. }));
        let started = std::time::Instant::now();
        shutdown.store(true, Ordering::Relaxed);
        wake.store(true, Ordering::Relaxed);
        worker.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test --lib sync::tests`
Expected: compile errors on the new `run_loop_with`/`run_loop` arity.

- [ ] **Step 4: Implement**

Add `pending_full: bool` to `AccountSync` (init `false` in `open`). Replace `pass`:

```rust
    /// Syncs every folder (`full`) or only INBOX, reloads the rules, runs them and sends the events. Queued
    /// commands run at the pass's checkpoints.
    fn pass(
        &mut self,
        ops: &mut dyn MailOps,
        full: bool,
        events: &Sender<Event>,
        commands: &Receiver<Command>,
        shutdown: &AtomicBool,
    ) -> Result<(), SyncError> {
        let (account, store, trash) = (self.account, &self.store, &self.trash);
        let activity = |activity| Event::Activity {
            account: account.name.clone(),
            activity,
        };
        let mut pending_full = false;
        let mut checkpoint = |ops: &mut dyn MailOps, step: Activity| {
            let _ = events.send(activity(step));
            if shutdown.load(Ordering::Relaxed) {
                return Err(SyncError::Stopped);
            }
            let run = run_commands(ops, store, trash, account, commands, events)?;
            pending_full |= run.wants_full_pass;
            Ok(run.used_connection)
        };
        let new = if full {
            let (new, sync_errors) = sync_all_with(ops, store, &mut checkpoint)?;
            for message in sync_errors {
                let _ = events.send(account_error(account, message));
            }
            new
        } else {
            let inbox = RemoteFolder {
                name: "INBOX".into(),
                special_use: None,
            };
            sync_folder_with(ops, store, &inbox, &mut checkpoint)?
        };
        self.pending_full |= pending_full;
        let previous = std::mem::take(&mut self.rules);
        self.rules = reload_rules(&self.store, &self.rules_path, now(), previous);
        let run = run_rules_with(
            ops,
            &self.store,
            &self.trash,
            &self.rules,
            self.account,
            &self.identity,
            &new,
            Mode::Normal,
            now(),
            &mut |step| {
                let _ = events.send(activity(step));
            },
        )?;
        for event in run.events {
            let _ = events.send(event);
        }
        let _ = events.send(Event::Synced {
            account: self.account.name.clone(),
            new_messages: new.len(),
            actions: run.actions,
        });
        Ok(())
    }
```

`run_once` passes a closed channel and a false flag:

```rust
    let (_, no_commands) = std::sync::mpsc::channel();
    AccountSync::open(account, paths)?.pass(&mut ops, true, events, &no_commands, &AtomicBool::new(false))
```

`run_loop`:

```rust
pub fn run_loop(
    account: AccountConfig,
    paths: Paths,
    events: Sender<Event>,
    shutdown: Arc<AtomicBool>,
    commands: Receiver<Command>,
    wake: Arc<AtomicBool>,
) {
    let name = account.name.clone();
    let (stop, woken) = (shutdown.clone(), wake.clone());
    run_loop_with(
        account.clone(),
        paths,
        events,
        shutdown,
        commands,
        wake,
        move || Ok(Box::new(connect(&account)?) as Box<dyn MailOps>),
        |delay| {
            log::warn!("{name}: reconnecting in {}s", delay.as_secs());
            // Stop or a new command cuts this wait short; clearing the flag keeps later waits at full length.
            let until = std::time::Instant::now() + delay;
            while std::time::Instant::now() < until
                && !stop.load(Ordering::Relaxed)
                && !woken.swap(false, Ordering::Relaxed)
            {
                std::thread::sleep(Duration::from_millis(100));
            }
        },
    );
}
```

`run_loop_with` gains `commands` and `wake` after `shutdown`, passes them to `run_session`, and its error arm becomes:

```rust
            Err(_) if shutdown.load(Ordering::Relaxed) => return,
            Err(e) => {
                let _ = events.send(Event::Error {
                    account: account.name.clone(),
                    message: e.to_string(),
                });
                let _ = events.send(Event::Activity {
                    account: account.name.clone(),
                    activity: Activity::Offline {
                        reason: e.to_string(),
                        retry_at: now() + backoff.as_secs() as i64,
                    },
                });
                sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(300));
            }
```

`run_session` gains `commands: &Receiver<Command>, wake: &AtomicBool` and becomes:

```rust
    let _ = events.send(Event::Activity {
        account: account.name.clone(),
        activity: Activity::Connecting,
    });
    let mut ops = connect()?;
    let mut state = AccountSync::open(account, paths)?;
    let interval = Duration::from_secs(account.sync_interval_secs.max(10));
    let mut last_purge = 0i64;
    let mut full = true;
    let mut pass = true;

    while !shutdown.load(Ordering::Relaxed) {
        if pass {
            state.pass(ops.as_mut(), full, events, commands, shutdown)?;
            if now() - last_purge > 3600 {
                /* the existing purge block, unchanged */
            }
        }
        let drained = run_commands(ops.as_mut(), &state.store, &state.trash, account, commands, events)?;
        if drained.wants_full_pass || std::mem::take(&mut state.pending_full) {
            (full, pass) = (true, true);
            continue;
        }
        let _ = events.send(Event::Activity {
            account: account.name.clone(),
            activity: Activity::Idle { since: now() },
        });
        ops.select("INBOX")?;
        let outcome = ops.idle(interval, wake)?;
        // A pass plus a wait means the connection is healthy; a pass alone does not, e.g. when IDLE always fails.
        *completed_cycle = true;
        (full, pass) = match outcome {
            IdleOutcome::NewMail => (false, true),
            IdleOutcome::Timeout => (true, true),
            IdleOutcome::Interrupted if shutdown.load(Ordering::Relaxed) => return Ok(()),
            // Woken for queued commands: run them, then wait again.
            IdleOutcome::Interrupted => {
                wake.store(false, Ordering::Relaxed);
                (full, false)
            }
        };
    }
    Ok(())
```

`wake` is cleared before the drain at the top of the next iteration, so a command queued after the drain sets it again and the next `idle` returns at once.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib sync::tests`
Expected: PASS, including `backoff_grows_caps_and_resets_after_a_full_cycle` with its sleep sequence unchanged.

- [ ] **Step 6: Make `cmd_run` compile** (Task 6 replaces it)

In `cmd_run`, give each spawned loop its own channel and flag:

```rust
        let (_commands_tx, commands) = mpsc::channel();
        let wake = Arc::new(AtomicBool::new(false));
        handles.push(
            std::thread::Builder::new()
                .name(format!("sync-{}", account.name))
                .spawn(move || sync::run_loop(account, paths, tx, shutdown, commands, wake))?,
        );
```

Dropping `_commands_tx` at the end of the loop iteration is fine: `try_recv` on a disconnected channel returns no commands.

- [ ] **Step 7: Run everything**

Run: `cargo test && POSTBODE_TEST_IMAP_HOST=localhost cargo test --test imap_live`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add src/sync.rs src/mail_ops.rs src/cli/mod.rs
git commit -m "feat(sync): drain commands between passes and wake IDLE for them"
```

---

### Task 6: `postbode::engine`, the account lock, shared notifications

**Files:**
- Create: `src/engine.rs`
- Create: `src/notify.rs`
- Modify: `src/lib.rs` (add `pub mod engine; pub mod notify;`, alphabetical)
- Modify: `src/message.rs` (receive `clean` and its test from the CLI)
- Modify: `src/cli/mod.rs` (`cmd_run` uses the engine; `clean`, `escape_markup`, `notification_text` and their tests move out; the `run` doc comment)
- Modify: `tests/cli.rs` (new test)
- Modify: `AGENTS.md` (module map), `docs/src/cli.md` (regenerated)

**Interfaces:**
- Consumes: Task 5 `run_loop(account, paths, events, shutdown, commands, wake)`, `Command`, `Event`.
- Produces:

```rust
// engine.rs
#[derive(Debug, Clone, PartialEq)]
pub enum StartState { Running, Locked { pid: Option<u32> }, Failed(String) }
pub struct Engine { /* private */ }
impl Engine {
    pub fn start(config: &Config, paths: &Paths) -> io::Result<(Engine, Receiver<Event>)>;
    pub fn accounts(&self) -> &[(String, StartState)];
    pub fn send(&self, account: &str, command: Command) -> bool;
    pub fn stop(self);
    pub fn detached(accounts: &[&str]) -> (Engine, Receiver<(String, Command)>);
}
// notify.rs
pub fn new_mail(from: &str, subject: &str);
// message.rs
pub fn clean(text: &str, keep_layout: bool) -> String;
```

  `start` returns `io::Result` rather than a new error enum: its only failure is spawning a thread; per-account lock failures become `StartState::Failed`.

- [ ] **Step 1: Write the failing tests**

`src/engine.rs` tests module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AccountConfig, PasswordSource};
    use crate::sync::Activity;

    fn offline_account(name: &str) -> AccountConfig {
        AccountConfig {
            name: name.into(),
            host: "127.0.0.1".into(),
            port: 1,
            username: "me@example.com".into(),
            password: PasswordSource::Command {
                command: "printf x".into(),
            },
            address: None,
            aliases: vec![],
            sync_interval_secs: 120,
            trash_retention_days: 30,
            notify: false,
            ca_file: None,
        }
    }

    #[test]
    fn an_account_locked_elsewhere_is_reported_and_not_started() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let held = lock_account(&paths, "work").unwrap().unwrap();
        let config = Config {
            accounts: vec![offline_account("work")],
        };
        let (engine, _events) = Engine::start(&config, &paths).unwrap();
        assert_eq!(
            engine.accounts(),
            [(
                "work".to_string(),
                StartState::Locked {
                    pid: Some(std::process::id())
                }
            )]
        );
        assert!(!engine.send("work", Command::SyncNow));
        engine.stop();
        drop(held);
        assert!(lock_account(&paths, "work").unwrap().is_ok());
    }

    #[test]
    fn stop_returns_promptly_while_an_account_is_offline() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            accounts: vec![offline_account("work")],
        };
        let (engine, events) = Engine::start(&config, &Paths::under(dir.path())).unwrap();
        assert_eq!(engine.accounts(), [("work".to_string(), StartState::Running)]);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if let Event::Activity { activity: Activity::Offline { .. }, .. } = events.recv_timeout(left).unwrap() {
                break;
            }
        }
        assert!(engine.send("work", Command::SyncNow));
        assert!(!engine.send("nope", Command::SyncNow));
        let started = std::time::Instant::now();
        engine.stop();
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn a_detached_engine_hands_commands_to_the_test() {
        let (engine, sent) = Engine::detached(&["work"]);
        assert_eq!(engine.accounts(), [("work".to_string(), StartState::Running)]);
        assert!(engine.send("work", Command::SyncNow));
        assert_eq!(sent.try_recv().unwrap(), ("work".to_string(), Command::SyncNow));
    }
}
```

`tests/cli.rs` (uses the existing `ONE_ACCOUNT` constant and `postbode` helper):

```rust
#[test]
fn run_exits_nonzero_when_every_account_is_synced_elsewhere() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("config")).unwrap();
    std::fs::write(home.path().join("config/config.toml"), ONE_ACCOUNT).unwrap();
    let dir = postbode::paths::Paths::under(home.path()).account_dir("work");
    std::fs::create_dir_all(&dir).unwrap();
    let lock = std::fs::File::create(dir.join("sync.lock")).unwrap();
    lock.try_lock().unwrap();
    let out = postbode(home.path(), &["run"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("already synced by another Postbode process"), "{stderr}");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib engine && cargo test --test cli run_exits_nonzero`
Expected: compile error (no `engine` module); after Step 3 compiles, the CLI test fails until Step 5.

- [ ] **Step 3: Implement `src/engine.rs`**

```rust
//! Runs one sync thread per account and routes commands to them; the entry point for front ends.
use std::collections::HashMap;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use crate::config::Config;
use crate::paths::Paths;
use crate::sync::{self, Command, Event};

#[derive(Debug, Clone, PartialEq)]
pub enum StartState {
    Running,
    /// Another process holds the account's lock; `pid` is what it wrote there.
    Locked { pid: Option<u32> },
    Failed(String),
}

struct AccountThread {
    commands: Sender<Command>,
    wake: Arc<AtomicBool>,
}

enum Route {
    Threads(HashMap<String, AccountThread>),
    Detached(Sender<(String, Command)>),
}

pub struct Engine {
    accounts: Vec<(String, StartState)>,
    route: Route,
    shutdown: Arc<AtomicBool>,
    handles: Vec<JoinHandle<()>>,
    _locks: Vec<File>,
}

impl Engine {
    /// Takes each account's lock and spawns its sync thread; accounts whose lock is held are not started.
    pub fn start(config: &Config, paths: &Paths) -> io::Result<(Engine, Receiver<Event>)> {
        let shutdown = Arc::new(AtomicBool::new(false));
        let (events, received) = mpsc::channel();
        let mut threads = HashMap::new();
        let mut engine = Engine {
            accounts: Vec::new(),
            route: Route::Threads(HashMap::new()),
            shutdown: shutdown.clone(),
            handles: Vec::new(),
            _locks: Vec::new(),
        };
        for account in &config.accounts {
            let name = account.name.clone();
            let lock = match lock_account(paths, &name) {
                Ok(Ok(file)) => file,
                Ok(Err(pid)) => {
                    engine.accounts.push((name, StartState::Locked { pid }));
                    continue;
                }
                Err(e) => {
                    engine.accounts.push((name, StartState::Failed(e.to_string())));
                    continue;
                }
            };
            let (commands_tx, commands) = mpsc::channel();
            let wake = Arc::new(AtomicBool::new(false));
            let handle = std::thread::Builder::new()
                .name(format!("sync-{name}"))
                .spawn({
                    let (account, paths, events) = (account.clone(), paths.clone(), events.clone());
                    let (shutdown, wake) = (shutdown.clone(), wake.clone());
                    move || sync::run_loop(account, paths, events, shutdown, commands, wake)
                })?;
            engine._locks.push(lock);
            engine.handles.push(handle);
            threads.insert(
                name.clone(),
                AccountThread {
                    commands: commands_tx,
                    wake,
                },
            );
            engine.accounts.push((name, StartState::Running));
        }
        engine.route = Route::Threads(threads);
        Ok((engine, received))
    }

    pub fn accounts(&self) -> &[(String, StartState)] {
        &self.accounts
    }

    /// Queues the command and wakes the account's IDLE; false when the account is not running.
    pub fn send(&self, account: &str, command: Command) -> bool {
        match &self.route {
            Route::Threads(threads) => threads.get(account).is_some_and(|thread| {
                let sent = thread.commands.send(command).is_ok();
                thread.wake.store(true, Ordering::Relaxed);
                sent
            }),
            Route::Detached(sent) => sent.send((account.to_string(), command)).is_ok(),
        }
    }

    /// Sets shutdown and every wake flag, then joins the threads.
    pub fn stop(self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Route::Threads(threads) = &self.route {
            for thread in threads.values() {
                thread.wake.store(true, Ordering::Relaxed);
            }
        }
        for handle in self.handles {
            let _ = handle.join();
        }
    }

    /// An engine with no threads whose commands arrive on the returned receiver, for front-end tests.
    pub fn detached(accounts: &[&str]) -> (Engine, Receiver<(String, Command)>) {
        let (sent, received) = mpsc::channel();
        let engine = Engine {
            accounts: accounts
                .iter()
                .map(|name| (name.to_string(), StartState::Running))
                .collect(),
            route: Route::Detached(sent),
            shutdown: Arc::new(AtomicBool::new(false)),
            handles: Vec::new(),
            _locks: Vec::new(),
        };
        (engine, received)
    }
}

/// The held lock file, or the pid written by the process holding it.
fn lock_account(paths: &Paths, name: &str) -> io::Result<Result<File, Option<u32>>> {
    paths.ensure_account(name)?;
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(paths.account_dir(name).join("sync.lock"))?;
    match file.try_lock() {
        Ok(()) => {
            file.set_len(0)?;
            write!(file, "{}", std::process::id())?;
            Ok(Ok(file))
        }
        Err(TryLockError::WouldBlock) => {
            let mut pid = String::new();
            file.read_to_string(&mut pid)?;
            Ok(Err(pid.trim().parse().ok()))
        }
        Err(TryLockError::Error(e)) => Err(e),
    }
}
```

- [ ] **Step 4: Move notification and cleaning helpers into the library**

Move `clean` (with its doc comment) from `src/cli/mod.rs` to `src/message.rs` as `pub fn clean`, and its test `clean_strips_control_characters` into `message.rs`'s tests. In `src/cli/mod.rs` add `use postbode::message::clean;` and delete the local function.

`src/notify.rs`:

```rust
//! Desktop notifications for new mail, shared by `run` and the GUI.
use crate::message::clean;

pub fn new_mail(from: &str, subject: &str) {
    let _ = notify_rust::Notification::new()
        .summary(&notification_text(from))
        .body(&notification_text(subject))
        .appname("Postbode")
        .show();
}

/// Linux notification servers render a subset of HTML in the summary and body.
fn escape_markup(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn notification_text(text: &str) -> String {
    let text = clean(text, false);
    if cfg!(target_os = "linux") {
        escape_markup(&text)
    } else {
        text
    }
}
```

Move the test `escape_markup_escapes_tags_and_entities` into a `#[cfg(test)] mod tests` in `notify.rs`; delete `escape_markup` and `notification_text` from the CLI.

- [ ] **Step 5: `cmd_run` uses the engine**

```rust
fn cmd_run(config: &Config, paths: &Paths) -> Result<()> {
    if config.accounts.is_empty() {
        bail!("no accounts configured; run `postbode account add`");
    }
    compiled_rules(paths, None)?;
    // Ctrl-C ends the process through the default SIGINT handler; WAL and trash-before-delete leave nothing half done.
    // `engine` holds the account locks until `run` exits.
    let (engine, events) = Engine::start(config, paths)?;
    let mut running = 0;
    for (name, state) in engine.accounts() {
        match state {
            StartState::Running => running += 1,
            StartState::Locked { pid } => eprintln!(
                "[{name}] already synced by another Postbode process{}; skipped",
                pid.map(|p| format!(" (pid {p})")).unwrap_or_default()
            ),
            StartState::Failed(e) => eprintln!("[{name}] could not start: {}", clean(e, false)),
        }
    }
    if running == 0 {
        bail!("no account could start");
    }
    for event in events {
        print_event(&event);
        if let Event::NewMail { from, subject, .. } = &event {
            postbode::notify::new_mail(from, subject);
        }
    }
    Ok(())
}
```

Remove the now-unused imports (`AtomicBool`, `Arc` if unused, `mpsc` stays for `Sync`). Change the `Run` doc comment to `/// Sync all accounts continuously and apply rules; an account another Postbode process syncs is skipped; Ctrl-C stops`.

- [ ] **Step 6: Docs**

In `AGENTS.md`'s module map, add alphabetically:
- `` `engine` one sync thread per account, the per-account lock, commands in and events out; what `run` and the GUI use``
- `` `notify` desktop notifications for new mail``

and change the `sync` line to `` `sync` per-account loop: sync folders in chunks, run rules, run commands, IDLE``.

Run: `POSTBODE_BLESS=1 cargo test` to regenerate `docs/src/cli.md`.

- [ ] **Step 7: Run everything**

Run: `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test && POSTBODE_TEST_IMAP_HOST=localhost cargo test --test imap_live`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add src/engine.rs src/notify.rs src/lib.rs src/message.rs src/cli/mod.rs tests/cli.rs AGENTS.md docs/src/cli.md
git commit -m "feat(engine): run accounts behind a lock and route commands to them"
```

---

### Task 7: `rules::edit::set_enabled`

**Files:**
- Modify: `src/rules/edit.rs`

**Interfaces:**
- Produces: `pub fn set_enabled(path: &Path, name: &str, enabled: bool) -> Result<(), RulesError>` — keeps comments and every other byte; `approve` keeps its "is already enabled" check and then writes through the same helper.

- [ ] **Step 1: Write the failing test** (in `src/rules/edit.rs` tests; use the module's existing temp-file helper if it has one)

```rust
    const TWO_RULES: &str = "# keep me\n[[rules]]\nname = \"a\"   # inline\nmatch.seen = true\nactions = [\"flag\"]\n\n[[rules]]\nname = \"b\"\nenabled = false # off for now\nmatch.seen = true\nactions = [\"flag\"]\n";

    #[test]
    fn set_enabled_toggles_one_rule_and_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.toml");
        std::fs::write(&path, TWO_RULES).unwrap();
        let b_section = |text: &str| text[text.find("[[rules]]\nname = \"b\"").unwrap()..].to_string();

        set_enabled(&path, "a", false).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# keep me\n[[rules]]\nname = \"a\"   # inline\n"), "{text}");
        assert!(text.contains("enabled = false"));
        assert_eq!(b_section(&text), b_section(TWO_RULES));

        set_enabled(&path, "b", true).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("enabled = true # off for now"), "{text}");
        assert!(set_enabled(&path, "missing", true).is_err());
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib rules::edit`
Expected: compile error — `set_enabled` not found.

- [ ] **Step 3: Implement**

```rust
/// Turns a rule on or off, keeping the file's comments and layout.
pub fn set_enabled(path: &Path, name: &str, enabled: bool) -> Result<(), RulesError> {
    edit(path, name, |rules, index| {
        write_enabled(rules.get_mut(index).expect("index comes from position()"), enabled);
        Ok(None)
    })
}

pub fn approve(path: &Path, name: &str) -> Result<(), RulesError> {
    edit(path, name, |rules, index| {
        let table = rules.get_mut(index).expect("index comes from position()");
        if !is_disabled(table) {
            return Err(invalid(name, "is already enabled"));
        }
        write_enabled(table, true);
        Ok(None)
    })
}

fn write_enabled(table: &mut toml_edit::Table, enabled: bool) {
    let mut value = Value::from(enabled);
    if let Some(old) = table.get("enabled").and_then(|item| item.as_value()) {
        *value.decor_mut() = old.decor().clone();
    }
    table["enabled"] = Item::Value(value);
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test --lib rules::edit`
Expected: PASS, including the existing approve/reject tests.

- [ ] **Step 5: Commit**

```bash
git add src/rules/edit.rs
git commit -m "feat(rules): add set_enabled for toggling a rule in place"
```

---

### Task 8: Live tests for chunked sync and command wake

**Files:**
- Modify: `tests/imap_live.rs`
- Modify: `docs/superpowers/specs/2026-10-06-postbode-core-design.md` §13 (one clause listing the two new live tests)

**Interfaces:**
- Consumes: `sync::run_once`, `Event::Activity`, `Activity::FetchingHeaders`, `engine::{Engine, StartState}`, `Command::Apply`, `rules::Action::MarkRead`.

- [ ] **Step 1: Write the tests**

```rust
use postbode::engine::Engine;
use postbode::rules::Action;
use postbode::sync::{Activity, Command};

#[test]
fn first_sync_of_a_large_folder_runs_in_chunks() {
    let Some(host) = host() else { return };
    let account = account(&host, PORT, "chunks");
    let mut ops = connect(&account);
    for i in 0..1200 {
        ops.append("INBOX", &mail(&format!("bulk {i}")), &[]).unwrap();
    }
    let home = tempfile::tempdir().unwrap();
    let paths = Paths::under(home.path());
    let (tx, rx) = std::sync::mpsc::channel();
    sync::run_once(&account, &paths, &tx).unwrap();
    drop(tx);
    let progress: Vec<usize> = rx
        .iter()
        .filter_map(|e| match e {
            Event::Activity {
                activity: Activity::FetchingHeaders { folder, done, total },
                ..
            } if folder == "INBOX" => {
                assert_eq!(total, 1200);
                Some(done)
            }
            _ => None,
        })
        .collect();
    assert_eq!(progress, [0, 500, 1000, 1200]);
    let store = Store::open(&paths.mail_db(&account.name)).unwrap();
    assert_eq!(store.message_count("INBOX").unwrap(), 1200);
}

#[test]
fn a_command_wakes_idle_and_runs_within_two_seconds() {
    let Some(host) = host() else { return };
    let account = account(&host, PORT, "wake");
    connect(&account).append("INBOX", &mail("wake me"), &[]).unwrap();
    let home = tempfile::tempdir().unwrap();
    let config = Config {
        accounts: vec![account.clone()],
    };
    let (engine, events) = Engine::start(&config, &Paths::under(home.path())).unwrap();
    let wait = |wanted: &dyn Fn(&Event) -> bool| loop {
        let event = events.recv_timeout(Duration::from_secs(30)).expect("event within 30 s");
        if wanted(&event) {
            return event;
        }
    };
    wait(&|e| matches!(e, Event::Activity { activity: Activity::Idle { .. }, .. }));
    let sent = Instant::now();
    assert!(engine.send(
        &account.name,
        Command::Apply {
            folder: "INBOX".into(),
            uids: vec![1],
            action: Action::MarkRead,
        }
    ));
    let done = wait(&|e| matches!(e, Event::ActionDone { .. }));
    assert!(sent.elapsed() < Duration::from_secs(2), "{:?}", sent.elapsed());
    assert!(matches!(done, Event::ActionDone { results, .. } if results == [(1, Ok(1))]));
    engine.stop();
}
```

Adjust the `use` lines to the file's existing imports (`Config`, `Duration`, `Instant`, `Paths`, `Store`, `Event`, `sync` are already imported).

- [ ] **Step 2: Run them**

Run: `docker compose -f tests/dovecot/compose.yml up -d && POSTBODE_TEST_IMAP_HOST=localhost cargo test --test imap_live`
Expected: PASS, 13 tests. Without the variable: `cargo test --test imap_live` passes with both skipped.

- [ ] **Step 3: Spec note**

In core spec §13's Dovecot bullet, extend the list "They cover …" with: "a 1,200-message first sync in chunks, a command waking IDLE through the engine,".

- [ ] **Step 4: Full definition of done**

Run: `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test && cargo machete && cargo audit`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add tests/imap_live.rs docs/superpowers/specs/2026-10-06-postbode-core-design.md
git commit -m "test(live): cover chunked first sync and command wake"
```
