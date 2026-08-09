//! SQLite-backed durable receipt store, mirroring `DurableReceiptStore.swift`.
//!
//! Durability delta versus macOS (documented in the README): Linux has no
//! F_FULLFSYNC, so `synchronous=FULL` relies on fsync-honoring storage, and
//! the HMAC key lives in a mode-0600 key file instead of the Keychain.

use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use hmac::{Hmac, Mac};
use rusqlite::{params, Connection, OpenFlags};
use sha2::Sha256;
use time::OffsetDateTime;

use computer_use_core::models::{ActionReceipt, ActionReceiptState, ComputerUseError, Result};
use computer_use_core::runtime::ActionReceiptStore;

const DATABASE_NAME: &str = "receipts-v2.sqlite3";
const KEY_FILE_NAME: &str = "receipt-hmac-v2.key";
const SIDECAR_SUFFIXES: [&str; 4] = ["", "-wal", "-shm", "-journal"];

pub struct DurableReceiptStore {
    connection: std::sync::Mutex<Connection>,
    authentication_key: Vec<u8>,
}

fn internal(message: &str) -> ComputerUseError {
    ComputerUseError::InternalFailure(message.to_owned())
}

fn denied(message: &str) -> ComputerUseError {
    ComputerUseError::PermissionDenied(message.to_owned())
}

impl DurableReceiptStore {
    pub fn open(directory: &Path) -> Result<Self> {
        prepare_private_directory(directory)?;
        let database_path = directory.join(DATABASE_NAME);
        prepare_database_file(&database_path)?;
        for suffix in &SIDECAR_SUFFIXES[1..] {
            secure_existing_file(&sidecar(&database_path, suffix))?;
        }
        let authentication_key = load_or_create_authentication_key(&directory.join(KEY_FILE_NAME))?;

        let connection = Connection::open_with_flags(
            &database_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_FULL_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|_| internal("Could not open the durable receipt database."))?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .ok();
        let pragmas = [
            "PRAGMA journal_mode=WAL",
            "PRAGMA synchronous=FULL",
            "PRAGMA secure_delete=ON",
            "PRAGMA wal_autocheckpoint=1",
        ];
        for pragma in pragmas {
            connection.execute_batch(pragma).map_err(|_| {
                internal("The receipt database could not enable its durability contract.")
            })?;
        }
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS action_receipts (
                    identifier TEXT PRIMARY KEY NOT NULL,
                    tool_name TEXT NOT NULL,
                    snapshot_identifier TEXT NOT NULL,
                    state TEXT NOT NULL,
                    created_at REAL NOT NULL,
                    updated_at REAL NOT NULL,
                    failure_code TEXT,
                    authentication BLOB NOT NULL
                ) STRICT;
                CREATE INDEX IF NOT EXISTS action_receipts_dispatched
                    ON action_receipts(updated_at)
                 WHERE state = 'dispatched';",
            )
            .map_err(|_| internal("The durable receipt database operation failed."))?;

        let journal_mode: String = connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .map_err(|_| {
                internal("The receipt database could not enable its durability contract.")
            })?;
        let synchronous: i64 = connection
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .map_err(|_| {
                internal("The receipt database could not enable its durability contract.")
            })?;
        if journal_mode.to_lowercase() != "wal" || synchronous != 2 {
            return Err(internal(
                "The receipt database could not enable its durability contract.",
            ));
        }
        for suffix in &SIDECAR_SUFFIXES {
            secure_existing_file(&sidecar(&database_path, suffix))?;
        }
        Ok(Self {
            connection: std::sync::Mutex::new(connection),
            authentication_key,
        })
    }

    fn authenticate(&self, receipt: &ActionReceipt) -> Vec<u8> {
        // Mirrors the macOS ReceiptAuthenticationPayload: canonical sorted-key
        // JSON over the receipt fields with timestamp bit patterns.
        let payload = serde_json::json!({
            "version": 2,
            "identifier": receipt.identifier,
            "toolName": receipt.tool_name,
            "snapshotIdentifier": receipt.snapshot_identifier,
            "state": receipt.state.as_str(),
            "createdAtBits": epoch_seconds(receipt.created_at).to_bits(),
            "updatedAtBits": epoch_seconds(receipt.updated_at).to_bits(),
            "failureCode": receipt.failure_code,
        });
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.authentication_key)
            .expect("HMAC accepts 32-byte keys");
        mac.update(
            serde_json::to_string(&payload)
                .expect("receipt payload serializes")
                .as_bytes(),
        );
        mac.finalize().into_bytes().to_vec()
    }

    fn decode_row(
        &self,
        row: &rusqlite::Row<'_>,
    ) -> std::result::Result<ActionReceipt, rusqlite::Error> {
        let state_name: String = row.get(3)?;
        let state = ActionReceiptState::parse(&state_name).ok_or(rusqlite::Error::InvalidQuery)?;
        Ok(ActionReceipt {
            identifier: row.get(0)?,
            tool_name: row.get(1)?,
            snapshot_identifier: row.get(2)?,
            state,
            created_at: from_epoch_seconds(row.get::<_, f64>(4)?),
            updated_at: from_epoch_seconds(row.get::<_, f64>(5)?),
            failure_code: row.get(6)?,
        })
    }

    fn load_locked(
        &self,
        connection: &Connection,
        identifier: &str,
    ) -> Result<Option<ActionReceipt>> {
        let mut statement = connection
            .prepare(
                "SELECT identifier, tool_name, snapshot_identifier, state, created_at, updated_at,
                        failure_code, authentication
                   FROM action_receipts WHERE identifier = ?1",
            )
            .map_err(|_| internal("The durable receipt database operation failed."))?;
        let mut rows = statement
            .query(params![identifier])
            .map_err(|_| internal("The durable receipt database operation failed."))?;
        let Some(row) = rows
            .next()
            .map_err(|_| internal("The durable receipt database operation failed."))?
        else {
            return Ok(None);
        };
        let receipt = self
            .decode_row(row)
            .map_err(|_| internal("The receipt database contains an invalid record."))?;
        let authentication: Vec<u8> = row
            .get(7)
            .map_err(|_| internal("The receipt database contains an invalid record."))?;
        if !constant_time_equal(&authentication, &self.authenticate(&receipt)) {
            return Err(internal("Receipt authentication failed."));
        }
        Ok(Some(receipt))
    }

    fn write_locked(
        &self,
        connection: &Connection,
        receipt: &ActionReceipt,
        insert: bool,
    ) -> Result<()> {
        let authentication = self.authenticate(receipt);
        let changed = if insert {
            connection.execute(
                "INSERT INTO action_receipts
                    (identifier, tool_name, snapshot_identifier, state, created_at, updated_at,
                     failure_code, authentication)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    receipt.identifier,
                    receipt.tool_name,
                    receipt.snapshot_identifier,
                    receipt.state.as_str(),
                    epoch_seconds(receipt.created_at),
                    epoch_seconds(receipt.updated_at),
                    receipt.failure_code,
                    authentication,
                ],
            )
        } else {
            connection.execute(
                "UPDATE action_receipts
                    SET state=?4, updated_at=?6, failure_code=?7, authentication=?8
                  WHERE identifier=?1 AND tool_name=?2 AND snapshot_identifier=?3 AND created_at=?5",
                params![
                    receipt.identifier,
                    receipt.tool_name,
                    receipt.snapshot_identifier,
                    receipt.state.as_str(),
                    epoch_seconds(receipt.created_at),
                    epoch_seconds(receipt.updated_at),
                    receipt.failure_code,
                    authentication,
                ],
            )
        }
        .map_err(|_| internal("The durable receipt database operation failed."))?;
        if changed != 1 {
            return Err(internal("The durable receipt database operation failed."));
        }
        Ok(())
    }
}

fn allowed_transition(from: ActionReceiptState, to: ActionReceiptState) -> bool {
    use ActionReceiptState::*;
    matches!(
        (from, to),
        (Prepared, Dispatched)
            | (Prepared, Rejected)
            | (Dispatched, Applied)
            | (Dispatched, Rejected)
            | (Dispatched, OutcomeUnknown)
    )
}

impl ActionReceiptStore for DurableReceiptStore {
    fn load(&self, identifier: &str) -> Result<Option<ActionReceipt>> {
        let connection = self.connection.lock().expect("receipt store lock");
        self.load_locked(&connection, identifier)
    }

    fn create(&self, receipt: &ActionReceipt) -> Result<()> {
        if receipt.state != ActionReceiptState::Prepared {
            return Err(internal("Invalid initial receipt state."));
        }
        let connection = self.connection.lock().expect("receipt store lock");
        connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|_| internal("The durable receipt database operation failed."))?;
        let outcome = (|| {
            if self
                .load_locked(&connection, &receipt.identifier)?
                .is_some()
            {
                return Err(internal(
                    "A receipt with this action identity already exists.",
                ));
            }
            self.write_locked(&connection, receipt, true)
        })();
        finish_transaction(&connection, outcome)
    }

    fn replace(&self, receipt: &ActionReceipt) -> Result<()> {
        let connection = self.connection.lock().expect("receipt store lock");
        connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|_| internal("The durable receipt database operation failed."))?;
        let outcome = (|| {
            let previous = self
                .load_locked(&connection, &receipt.identifier)?
                .ok_or_else(|| internal("Invalid receipt state transition."))?;
            if previous.tool_name != receipt.tool_name
                || previous.snapshot_identifier != receipt.snapshot_identifier
                || !allowed_transition(previous.state, receipt.state)
            {
                return Err(internal("Invalid receipt state transition."));
            }
            self.write_locked(&connection, receipt, false)
        })();
        finish_transaction(&connection, outcome)
    }

    fn recover_dispatched(&self, at: OffsetDateTime) -> Result<()> {
        let connection = self.connection.lock().expect("receipt store lock");
        connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|_| internal("The durable receipt database operation failed."))?;
        let outcome = (|| {
            let mut statement = connection
                .prepare(
                    "SELECT identifier, tool_name, snapshot_identifier, state, created_at,
                            updated_at, failure_code, authentication
                       FROM action_receipts WHERE state = 'dispatched'",
                )
                .map_err(|_| internal("The durable receipt database operation failed."))?;
            let mut stranded = Vec::new();
            let mut rows = statement
                .query([])
                .map_err(|_| internal("The durable receipt database operation failed."))?;
            while let Some(row) = rows
                .next()
                .map_err(|_| internal("The durable receipt database operation failed."))?
            {
                let receipt = self
                    .decode_row(row)
                    .map_err(|_| internal("The receipt database contains an invalid record."))?;
                let authentication: Vec<u8> = row
                    .get(7)
                    .map_err(|_| internal("The receipt database contains an invalid record."))?;
                if !constant_time_equal(&authentication, &self.authenticate(&receipt)) {
                    return Err(internal("Receipt authentication failed."));
                }
                stranded.push(receipt);
            }
            drop(rows);
            drop(statement);
            for receipt in stranded {
                let recovered = receipt.transitioned(
                    ActionReceiptState::OutcomeUnknown,
                    at,
                    Some("agent_restart".to_owned()),
                );
                self.write_locked(&connection, &recovered, false)?;
            }
            Ok(())
        })();
        finish_transaction(&connection, outcome)
    }
}

fn finish_transaction(connection: &Connection, outcome: Result<()>) -> Result<()> {
    match outcome {
        Ok(()) => connection
            .execute_batch("COMMIT")
            .map_err(|_| internal("The durable receipt database operation failed.")),
        Err(error) => {
            let _ = connection.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

fn epoch_seconds(date: OffsetDateTime) -> f64 {
    date.unix_timestamp_nanos() as f64 / 1e9
}

fn from_epoch_seconds(seconds: f64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp_nanos((seconds * 1e9) as i128)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

fn sidecar(database_path: &Path, suffix: &str) -> PathBuf {
    let mut name = database_path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right.iter())
            .fold(0u8, |accumulator, (a, b)| accumulator | (a ^ b))
            == 0
}

/// Creates (mode 0700) and validates a private, user-owned, non-symlinked
/// directory, mirroring the macOS `preparePrivateDirectory`.
pub fn prepare_private_directory(directory: &Path) -> Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    if let Some(parent) = directory.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|_| denied("The receipt directory is not a private user-owned directory."))?;
    }
    let path = CString::new(directory.as_os_str().as_bytes())
        .map_err(|_| denied("The receipt directory is not a private user-owned directory."))?;
    let created = unsafe { libc::mkdir(path.as_ptr(), 0o700) };
    if created != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST) {
        return Err(denied(
            "The receipt directory is not a private user-owned directory.",
        ));
    }
    let descriptor = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        return Err(denied(
            "The receipt directory is not a private user-owned directory.",
        ));
    }
    let guard = FdGuard(descriptor);
    let mut descriptor_status: libc::stat = unsafe { std::mem::zeroed() };
    let mut path_status: libc::stat = unsafe { std::mem::zeroed() };
    let valid = unsafe {
        libc::fstat(guard.0, &mut descriptor_status) == 0
            && libc::lstat(path.as_ptr(), &mut path_status) == 0
    } && descriptor_status.st_dev == path_status.st_dev
        && descriptor_status.st_ino == path_status.st_ino
        && descriptor_status.st_uid == unsafe { libc::geteuid() }
        && descriptor_status.st_mode & libc::S_IFMT == libc::S_IFDIR
        && unsafe { libc::fchmod(guard.0, 0o700) } == 0;
    if !valid {
        return Err(denied(
            "The receipt directory is not a private user-owned directory.",
        ));
    }
    Ok(())
}

struct FdGuard(i32);

impl Drop for FdGuard {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

fn open_no_follow(path: &Path, flags: i32, mode: u32) -> Result<FdGuard> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| denied("A durable receipt state file could not be opened safely."))?;
    let descriptor = unsafe {
        libc::open(
            path.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            mode,
        )
    };
    if descriptor < 0 {
        return Err(denied(
            "A durable receipt state file could not be opened safely.",
        ));
    }
    Ok(FdGuard(descriptor))
}

fn secure_descriptor(guard: &FdGuard, path: &Path) -> Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| denied("A durable receipt state file is not private and user-owned."))?;
    let mut descriptor_status: libc::stat = unsafe { std::mem::zeroed() };
    let mut path_status: libc::stat = unsafe { std::mem::zeroed() };
    let valid = unsafe {
        libc::fstat(guard.0, &mut descriptor_status) == 0
            && libc::lstat(path.as_ptr(), &mut path_status) == 0
    } && descriptor_status.st_dev == path_status.st_dev
        && descriptor_status.st_ino == path_status.st_ino
        && descriptor_status.st_uid == unsafe { libc::geteuid() }
        && descriptor_status.st_mode & libc::S_IFMT == libc::S_IFREG
        && descriptor_status.st_nlink == 1
        && unsafe { libc::fchmod(guard.0, 0o600) } == 0;
    if !valid {
        return Err(denied(
            "A durable receipt state file is not private and user-owned.",
        ));
    }
    Ok(())
}

fn prepare_database_file(path: &Path) -> Result<()> {
    let guard = open_no_follow(path, libc::O_CREAT | libc::O_RDWR, 0o600)?;
    secure_descriptor(&guard, path)
}

fn secure_existing_file(path: &Path) -> Result<()> {
    match open_no_follow(path, libc::O_RDONLY, 0) {
        Ok(guard) => secure_descriptor(&guard, path),
        Err(_) if !path.exists() => Ok(()),
        Err(error) => Err(error),
    }
}

/// Loads or creates the 32-byte receipt HMAC key in a mode-0600 file. This is
/// the Linux stand-in for the macOS Keychain item and is intentionally the
/// weakest link the deployment model accepts (same-user file access).
fn load_or_create_authentication_key(path: &Path) -> Result<Vec<u8>> {
    if path.exists() {
        let guard = open_no_follow(path, libc::O_RDONLY, 0)?;
        secure_descriptor(&guard, path)?;
        let mut file = unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(guard.0) };
        std::mem::forget(guard);
        use std::io::Read as _;
        let mut encoded = String::new();
        file.read_to_string(&mut encoded)
            .map_err(|_| internal("Could not load the receipt authentication key."))?;
        let key = base64::engine::general_purpose::STANDARD
            .decode(encoded.trim())
            .map_err(|_| internal("Could not load the receipt authentication key."))?;
        if key.len() != 32 {
            return Err(internal("Could not load the receipt authentication key."));
        }
        return Ok(key);
    }
    let mut key = vec![0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut key);
    let guard = open_no_follow(path, libc::O_CREAT | libc::O_EXCL | libc::O_WRONLY, 0o600)?;
    secure_descriptor(&guard, path)?;
    let mut file = unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(guard.0) };
    std::mem::forget(guard);
    use std::io::Write as _;
    file.write_all(
        base64::engine::general_purpose::STANDARD
            .encode(&key)
            .as_bytes(),
    )
    .and_then(|_| file.sync_all())
    .map_err(|_| internal("Could not persist the receipt authentication key."))?;
    let _ = file.as_raw_fd();
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(identifier: &str, state: ActionReceiptState) -> ActionReceipt {
        let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        ActionReceipt {
            identifier: identifier.to_owned(),
            tool_name: "click".to_owned(),
            snapshot_identifier: "snap-1".to_owned(),
            state,
            created_at: now,
            updated_at: now,
            failure_code: None,
        }
    }

    #[test]
    fn receipts_round_trip_with_authentication() {
        let directory = tempfile::tempdir().unwrap();
        let store = DurableReceiptStore::open(&directory.path().join("receipts")).unwrap();
        let prepared = receipt("a-1", ActionReceiptState::Prepared);
        store.create(&prepared).unwrap();
        let loaded = store.load("a-1").unwrap().unwrap();
        assert_eq!(loaded, prepared);
        assert!(store.load("missing").unwrap().is_none());
    }

    #[test]
    fn transitions_are_enforced() {
        let directory = tempfile::tempdir().unwrap();
        let store = DurableReceiptStore::open(&directory.path().join("receipts")).unwrap();
        let prepared = receipt("a-1", ActionReceiptState::Prepared);
        store.create(&prepared).unwrap();
        // prepared -> applied is illegal.
        let now = prepared.created_at;
        assert!(store
            .replace(&prepared.transitioned(ActionReceiptState::Applied, now, None))
            .is_err());
        store
            .replace(&prepared.transitioned(ActionReceiptState::Dispatched, now, None))
            .unwrap();
        store
            .replace(&prepared.transitioned(ActionReceiptState::Applied, now, None))
            .unwrap();
        // applied is terminal.
        assert!(store
            .replace(&prepared.transitioned(ActionReceiptState::Rejected, now, None))
            .is_err());
        // duplicate create fails.
        assert!(store.create(&prepared).is_err());
    }

    #[test]
    fn recovery_marks_dispatched_outcome_unknown_across_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("receipts");
        let prepared = receipt("a-1", ActionReceiptState::Prepared);
        {
            let store = DurableReceiptStore::open(&path).unwrap();
            store.create(&prepared).unwrap();
            store
                .replace(&prepared.transitioned(
                    ActionReceiptState::Dispatched,
                    prepared.created_at,
                    None,
                ))
                .unwrap();
        }
        let store = DurableReceiptStore::open(&path).unwrap();
        store.recover_dispatched(prepared.created_at).unwrap();
        let recovered = store.load("a-1").unwrap().unwrap();
        assert_eq!(recovered.state, ActionReceiptState::OutcomeUnknown);
        assert_eq!(recovered.failure_code.as_deref(), Some("agent_restart"));
    }

    #[test]
    fn tampered_rows_fail_authentication() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("receipts");
        let store = DurableReceiptStore::open(&path).unwrap();
        store
            .create(&receipt("a-1", ActionReceiptState::Prepared))
            .unwrap();
        {
            let connection = store.connection.lock().unwrap();
            connection
                .execute("UPDATE action_receipts SET tool_name='type_text'", [])
                .unwrap();
        }
        assert!(store.load("a-1").is_err());
    }
}
