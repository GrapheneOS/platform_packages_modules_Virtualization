// Copyright 2024, The Android Open Source Project
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Database of VM IDs.

use anyhow::{anyhow, Context, Result};
use log::{debug, error, info, warn};
use rusqlite::{params, params_from_iter, Connection, OpenFlags, Rows};
use std::path::PathBuf;

/// Subdirectory to hold the database.
const DB_DIR: &str = "vmdb";

/// Name of the file that holds the database.
const DB_FILENAME: &str = "vmids.sqlite";

/// Maximum number of host parameters in a single SQL statement.
/// (Default value of `SQLITE_LIMIT_VARIABLE_NUMBER` for <= 3.32.0)
const MAX_VARIABLES: usize = 999;

/// Return the current time as milliseconds since epoch.
fn db_now() -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(std::time::Duration::ZERO)
        .as_millis();
    now.try_into().unwrap_or(u64::MAX)
}

/// Identifier for a VM and its corresponding secret.
pub type VmId = [u8; 64];

/// Representation of an on-disk database of VM IDs.
pub struct VmIdDb {
    conn: Connection,
}

struct RetryOnFailure(bool);

impl VmIdDb {
    /// Connect to the VM ID database file held in the given directory, creating it if necessary.
    /// The second return value indicates whether a new database file was created.
    ///
    /// This function assumes no other threads/processes are attempting to connect concurrently.
    pub fn new(db_dir: &str) -> Result<(Self, bool)> {
        let mut db_path = PathBuf::from(db_dir);
        db_path.push(DB_DIR);
        if !db_path.exists() {
            std::fs::create_dir(&db_path).context("failed to create {db_path:?}")?;
            info!("created persistent db dir {db_path:?}");
        }
        db_path.push(DB_FILENAME);
        Self::new_at_path(db_path, RetryOnFailure(true))
    }

    fn new_at_path(db_path: PathBuf, retry: RetryOnFailure) -> Result<(Self, bool)> {
        let (flags, created) = if db_path.exists() {
            debug!("connecting to existing database {db_path:?}");
            (
                OpenFlags::SQLITE_OPEN_READ_WRITE
                    | OpenFlags::SQLITE_OPEN_URI
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                false,
            )
        } else {
            info!("creating fresh database {db_path:?}");
            (
                OpenFlags::SQLITE_OPEN_READ_WRITE
                    | OpenFlags::SQLITE_OPEN_CREATE
                    | OpenFlags::SQLITE_OPEN_URI
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                true,
            )
        };
        let mut db = Self {
            conn: Connection::open_with_flags(&db_path, flags)
                .context(format!("failed to open/create DB with {flags:?}"))?,
        };

        if created {
            db.init_tables().context("failed to create tables")?;
        } else {
            // An existing .sqlite file may have an earlier schema.
            match db.schema_version() {
                Err(e) => {
                    // Couldn't determine a schema version, so wipe and try again.
                    error!("failed to determine VM DB schema: {e:?}");
                    if retry.0 {
                        // This is the first attempt, so wipe and retry.
                        error!("resetting database file {db_path:?}");
                        let _ = std::fs::remove_file(&db_path);
                        return Self::new_at_path(db_path, RetryOnFailure(false));
                    } else {
                        // An earlier attempt at wiping/retrying has failed, so give up.
                        return Err(anyhow!("failed to reset database file {db_path:?}"));
                    }
                }
                Ok(0) => db.upgrade_tables_v0_v1().context("failed to upgrade schema v0 -> v1")?,
                Ok(1) => {
                    // Current version, no action needed.
                }
                Ok(version) => {
                    // If the database looks like it's from a future version, leave it alone and
                    // fail to connect to it.
                    error!("database from the future (v{version})");
                    return Err(anyhow!("database from the future (v{version})"));
                }
            }
        }
        Ok((db, created))
    }

    /// Delete the associated database file.
    pub fn delete_db_file(self, db_dir: &str) {
        let mut db_path = PathBuf::from(db_dir);
        db_path.push(DB_DIR);
        db_path.push(DB_FILENAME);

        // Drop the connection before removing the backing file.
        drop(self);
        warn!("removing database file {db_path:?}");
        if let Err(e) = std::fs::remove_file(&db_path) {
            error!("failed to remove database file {db_path:?}: {e:?}");
        }
    }

    fn schema_version(&mut self) -> Result<i32> {
        let version: i32 = self
            .conn
            .query_row("PRAGMA main.user_version", (), |row| row.get(0))
            .context("failed to read pragma")?;
        Ok(version)
    }

    /// Create the database table and indices using the current schema.
    fn init_tables(&mut self) -> Result<()> {
        self.init_tables_v1()
    }

    /// Create the database table and indices using the v1 schema.
    fn init_tables_v1(&mut self) -> Result<()> {
        info!("creating v1 database schema");
        self.conn
            .execute(
                "CREATE TABLE IF NOT EXISTS main.vmids (
                     vm_id BLOB PRIMARY KEY,
                     user_id INTEGER,
                     app_id INTEGER,
                     created INTEGER
                 ) WITHOUT ROWID;",
                (),
            )
            .context("failed to create table")?;
        self.conn
            .execute("CREATE INDEX IF NOT EXISTS main.vmids_user_index ON vmids(user_id);", [])
            .context("Failed to create user index")?;
        self.conn
            .execute(
                "CREATE INDEX IF NOT EXISTS main.vmids_app_index ON vmids(user_id, app_id);",
                [],
            )
            .context("Failed to create app index")?;
        self.conn
            .execute("PRAGMA main.user_version = 1;", ())
            .context("failed to declare version")?;
        Ok(())
    }

    fn upgrade_tables_v0_v1(&mut self) -> Result<()> {
        let _rows = self
            .conn
            .execute("ALTER TABLE main.vmids ADD COLUMN created INTEGER;", ())
            .context("failed to alter table v0->v1")?;
        self.conn
            .execute("PRAGMA main.user_version = 1;", ())
            .context("failed to set schema version")?;
        Ok(())
    }

    /// Create the database table and indices using the v0 schema.
    #[cfg(test)]
    fn init_tables_v0(&mut self) -> Result<()> {
        info!("creating v0 database schema");
        self.conn
            .execute(
                "CREATE TABLE IF NOT EXISTS main.vmids (
                     vm_id BLOB PRIMARY KEY,
                     user_id INTEGER,
                     app_id INTEGER
                 ) WITHOUT ROWID;",
                (),
            )
            .context("failed to create table")?;
        self.conn
            .execute("CREATE INDEX IF NOT EXISTS main.vmids_user_index ON vmids(user_id);", [])
            .context("Failed to create user index")?;
        self.conn
            .execute(
                "CREATE INDEX IF NOT EXISTS main.vmids_app_index ON vmids(user_id, app_id);",
                [],
            )
            .context("Failed to create app index")?;
        Ok(())
    }

    /// Add the given VM ID into the database.
    pub fn add_vm_id(&mut self, vm_id: &VmId, user_id: i32, app_id: i32) -> Result<()> {
        let now = db_now();
        let _rows = self
            .conn
            .execute(
                "REPLACE INTO main.vmids (vm_id, user_id, app_id, created) VALUES (?1, ?2, ?3, ?4);",
                params![vm_id, &user_id, &app_id, &now],
            )
            .context("failed to add VM ID")?;
        Ok(())
    }

    /// Remove the given VM IDs from the database.  The collection of IDs is assumed to be smaller
    /// than the maximum number of SQLite parameters.
    pub fn delete_vm_ids(&mut self, vm_ids: &[VmId]) -> Result<()> {
        assert!(vm_ids.len() < MAX_VARIABLES);
        let mut vars = "?,".repeat(vm_ids.len());
        vars.pop(); // remove trailing comma
        let sql = format!("DELETE FROM main.vmids WHERE vm_id IN ({});", vars);
        let mut stmt = self.conn.prepare(&sql).context("failed to prepare DELETE stmt")?;
        let _rows = stmt.execute(params_from_iter(vm_ids)).context("failed to delete VM IDs")?;
        Ok(())
    }

    /// Return the VM IDs associated with Android user ID `user_id`.
    pub fn vm_ids_for_user(&mut self, user_id: i32) -> Result<Vec<VmId>> {
        let mut stmt = self
            .conn
            .prepare("SELECT vm_id FROM main.vmids WHERE user_id = ?;")
            .context("failed to prepare SELECT stmt")?;
        let rows = stmt.query(params![user_id]).context("query failed")?;
        Self::vm_ids_from_rows(rows)
    }

    /// Return the VM IDs associated with `(user_id, app_id)`.
    pub fn vm_ids_for_app(&mut self, user_id: i32, app_id: i32) -> Result<Vec<VmId>> {
        let mut stmt = self
            .conn
            .prepare("SELECT vm_id FROM main.vmids WHERE user_id = ? AND app_id = ?;")
            .context("failed to prepare SELECT stmt")?;
        let rows = stmt.query(params![user_id, app_id]).context("query failed")?;
        Self::vm_ids_from_rows(rows)
    }

    /// Retrieve a collection of VM IDs from database rows.
    fn vm_ids_from_rows(mut rows: Rows) -> Result<Vec<VmId>> {
        let mut vm_ids: Vec<VmId> = Vec::new();
        while let Some(row) = rows.next().context("failed row unpack")? {
            match row.get(0) {
                Ok(vm_id) => vm_ids.push(vm_id),
                Err(e) => error!("failed to parse row: {e:?}"),
            }
        }

        Ok(vm_ids)
    }

    /// Determine whether the specified VM ID is associated with `(user_id, app_id)`. Returns false
    /// if there is no such VM ID, or it exists but is not associated.
    pub fn is_vm_id_for_app(&mut self, vm_id: &VmId, user_id: u32, app_id: u32) -> Result<bool> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT COUNT(*) FROM main.vmids \
                        WHERE vm_id = ? AND user_id = ? AND app_id = ?;",
            )
            .context("failed to prepare SELECT stmt")?;
        stmt.query_row(params![vm_id, user_id, app_id], |row| row.get(0))
            .context("query failed")
            .map(|n: usize| n != 0)
    }

    /// Determine the number of VM IDs associated with `(user_id, app_id)`.
    pub fn count_vm_ids_for_app(&mut self, user_id: i32, app_id: i32) -> Result<usize> {
        let mut stmt = self
            .conn
            .prepare("SELECT COUNT(vm_id) FROM main.vmids WHERE user_id = ? AND app_id = ?;")
            .context("failed to prepare SELECT stmt")?;
        stmt.query_row(params![user_id, app_id], |row| row.get(0)).context("query failed")
    }

    /// Return the `count` oldest VM IDs associated with `(user_id, app_id)`.
    pub fn oldest_vm_ids_for_app(
        &mut self,
        user_id: i32,
        app_id: i32,
        count: usize,
    ) -> Result<Vec<VmId>> {
        // SQLite considers NULL columns to be smaller than values, so rows left over from a v0
        // database will be listed first.
        let mut stmt = self
            .conn
            .prepare(
                "SELECT vm_id FROM main.vmids WHERE user_id = ? AND app_id = ? ORDER BY created LIMIT ?;",
            )
            .context("failed to prepare SELECT stmt")?;
        let rows = stmt.query(params![user_id, app_id, count]).context("query failed")?;
        Self::vm_ids_from_rows(rows)
    }

    /// Return all of the `(user_id, app_id)` pairs present in the database.
    pub fn get_all_owners(&mut self) -> Result<Vec<(i32, i32)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT user_id, app_id FROM main.vmids;")
            .context("failed to prepare SELECT stmt")?;
        let mut rows = stmt.query(()).context("query failed")?;
        let mut owners: Vec<(i32, i32)> = Vec::new();
        while let Some(row) = rows.next().context("failed row unpack")? {
            let user_id = match row.get(0) {
                Ok(v) => v,
                Err(e) => {
                    error!("failed to parse row: {e:?}");
                    continue;
                }
            };
            let app_id = match row.get(1) {
                Ok(v) => v,
                Err(e) => {
                    error!("failed to parse row: {e:?}");
                    continue;
                }
            };
            owners.push((user_id, app_id));
        }

        Ok(owners)
    }
}

/// Current schema version.
#[cfg(test)]
const SCHEMA_VERSION: usize = 1;

/// Create a new in-memory database for testing.
#[cfg(test)]
pub fn new_test_db() -> VmIdDb {
    tests::new_test_db_version(SCHEMA_VERSION)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const VM_ID1: VmId = [1u8; 64];
    const VM_ID2: VmId = [2u8; 64];
    const VM_ID3: VmId = [3u8; 64];
    const VM_ID4: VmId = [4u8; 64];
    const VM_ID5: VmId = [5u8; 64];
    const VM_ID_UNKNOWN: VmId = [6u8; 64];
    const USER1: i32 = 1;
    const USER2: i32 = 2;
    const USER3: i32 = 3;
    const USER_UNKNOWN: i32 = 4;
    const APP_A: i32 = 50;
    const APP_B: i32 = 60;
    const APP_C: i32 = 70;
    const APP_UNKNOWN: i32 = 99;

    pub fn new_test_db_version(version: usize) -> VmIdDb {
        let mut db = VmIdDb { conn: Connection::open_in_memory().unwrap() };
        match version {
            0 => db.init_tables_v0().unwrap(),
            1 => db.init_tables_v1().unwrap(),
            _ => panic!("unexpected version {version}"),
        }
        db
    }

    fn show_contents(db: &VmIdDb) {
        let mut stmt = db.conn.prepare("SELECT * FROM main.vmids;").unwrap();
        let mut rows = stmt.query(()).unwrap();
        println!("DB contents:");
        while let Some(row) = rows.next().unwrap() {
            println!("  {row:?}");
        }
    }

    fn show_contents_for_app(db: &VmIdDb, user_id: i32, app_id: i32, count: usize) {
        let mut stmt = db
            .conn
            .prepare("SELECT vm_id, created FROM main.vmids WHERE user_id = ? AND app_id = ? ORDER BY created LIMIT ?;")
            .unwrap();
        let mut rows = stmt.query(params![user_id, app_id, count]).unwrap();
        println!("First (by created) {count} rows for app_id={app_id}");
        while let Some(row) = rows.next().unwrap() {
            println!("  {row:?}");
        }
    }

    #[test]
    fn test_schema_version0() {
        let mut db0 = VmIdDb { conn: Connection::open_in_memory().unwrap() };
        db0.init_tables_v0().unwrap();
        let version = db0.schema_version().unwrap();
        assert_eq!(0, version);
    }

    #[test]
    fn test_schema_version1() {
        let mut db1 = VmIdDb { conn: Connection::open_in_memory().unwrap() };
        db1.init_tables_v1().unwrap();
        let version = db1.schema_version().unwrap();
        assert_eq!(1, version);
    }

    #[test]
    fn test_schema_upgrade_v0_v1() {
        let mut db = new_test_db_version(0);
        let version = db.schema_version().unwrap();
        assert_eq!(0, version);

        // Manually insert a row before upgrade.
        db.conn
            .execute(
                "REPLACE INTO main.vmids (vm_id, user_id, app_id) VALUES (?1, ?2, ?3);",
                params![&VM_ID1, &USER1, APP_A],
            )
            .unwrap();

        db.upgrade_tables_v0_v1().unwrap();
        let version = db.schema_version().unwrap();
        assert_eq!(1, version);

        assert_eq!(vec![VM_ID1], db.vm_ids_for_user(USER1).unwrap());
        show_contents(&db);
    }

    #[test]
    fn test_corrupt_database_file() {
        let db_dir = tempfile::Builder::new().prefix("vmdb-test-").tempdir().unwrap();
        let mut db_path = db_dir.path().to_owned();
        db_path.push(DB_FILENAME);
        {
            let mut file = std::fs::File::create(db_path).unwrap();
            let _ = file.write_all(b"This is not an SQLite file!");
        }

        // Non-DB file should be wiped and start over.
        let (mut db, created) =
            VmIdDb::new(&db_dir.path().to_string_lossy()).expect("failed to replace bogus DB");
        assert!(created);
        db.add_vm_id(&VM_ID1, USER1, APP_A).unwrap();
        assert_eq!(vec![VM_ID1], db.vm_ids_for_user(USER1).unwrap());
    }

    #[test]
    fn test_non_upgradable_database_file() {
        let db_dir = tempfile::Builder::new().prefix("vmdb-test-").tempdir().unwrap();
        let mut db_path = db_dir.path().to_owned();
        db_path.push(DB_FILENAME);
        {
            // Create an unrelated database that happens to apparently have a schema version of 0.
            let (db, created) = VmIdDb::new(&db_dir.path().to_string_lossy()).unwrap();
            assert!(created);
            db.conn.execute("DROP TABLE main.vmids", ()).unwrap();
            db.conn.execute("PRAGMA main.user_version = 0;", ()).unwrap();
        }

        // Should fail to open a database because the upgrade fails.
        let result = VmIdDb::new(&db_dir.path().to_string_lossy());
        assert!(result.is_err());
    }

    #[test]
    fn test_database_from_the_future() {
        let db_dir = tempfile::Builder::new().prefix("vmdb-test-").tempdir().unwrap();
        {
            let (mut db, created) = VmIdDb::new(&db_dir.path().to_string_lossy()).unwrap();
            assert!(created);
            db.add_vm_id(&VM_ID1, USER1, APP_A).unwrap();
            // Make the database look like it's from a future version.
            db.conn.execute("PRAGMA main.user_version = 99;", ()).unwrap();
        }
        // Should fail to open a database from the future.
        let result = VmIdDb::new(&db_dir.path().to_string_lossy());
        assert!(result.is_err());
    }

    #[test]
    fn test_add_remove() {
        let mut db = new_test_db();
        db.add_vm_id(&VM_ID1, USER1, APP_A).unwrap();
        db.add_vm_id(&VM_ID2, USER1, APP_A).unwrap();
        db.add_vm_id(&VM_ID3, USER1, APP_A).unwrap();
        db.add_vm_id(&VM_ID4, USER2, APP_B).unwrap();
        db.add_vm_id(&VM_ID5, USER3, APP_A).unwrap();
        db.add_vm_id(&VM_ID5, USER3, APP_C).unwrap(); // Overwrites APP_A

        assert_eq!(
            vec![(USER1, APP_A), (USER2, APP_B), (USER3, APP_C)],
            db.get_all_owners().unwrap()
        );

        let empty: Vec<VmId> = Vec::new();

        assert_eq!(vec![VM_ID1, VM_ID2, VM_ID3], db.vm_ids_for_user(USER1).unwrap());
        assert_eq!(vec![VM_ID1, VM_ID2, VM_ID3], db.vm_ids_for_app(USER1, APP_A).unwrap());
        assert_eq!(3, db.count_vm_ids_for_app(USER1, APP_A).unwrap());
        assert_eq!(vec![VM_ID4], db.vm_ids_for_app(USER2, APP_B).unwrap());
        assert_eq!(1, db.count_vm_ids_for_app(USER2, APP_B).unwrap());
        assert_eq!(vec![VM_ID5], db.vm_ids_for_user(USER3).unwrap());
        assert_eq!(empty, db.vm_ids_for_user(USER_UNKNOWN).unwrap());
        assert_eq!(empty, db.vm_ids_for_app(USER1, APP_UNKNOWN).unwrap());
        assert_eq!(0, db.count_vm_ids_for_app(USER1, APP_UNKNOWN).unwrap());

        assert!(db.is_vm_id_for_app(&VM_ID1, USER1 as u32, APP_A as u32).unwrap());
        assert!(!db.is_vm_id_for_app(&VM_ID1, USER2 as u32, APP_A as u32).unwrap());
        assert!(!db.is_vm_id_for_app(&VM_ID1, USER1 as u32, APP_B as u32).unwrap());
        assert!(!db.is_vm_id_for_app(&VM_ID_UNKNOWN, USER1 as u32, APP_A as u32).unwrap());
        assert!(!db.is_vm_id_for_app(&VM_ID5, USER3 as u32, APP_A as u32).unwrap());
        assert!(db.is_vm_id_for_app(&VM_ID5, USER3 as u32, APP_C as u32).unwrap());

        db.delete_vm_ids(&[VM_ID2, VM_ID3]).unwrap();

        assert_eq!(vec![VM_ID1], db.vm_ids_for_user(USER1).unwrap());
        assert_eq!(vec![VM_ID1], db.vm_ids_for_app(USER1, APP_A).unwrap());
        assert_eq!(1, db.count_vm_ids_for_app(USER1, APP_A).unwrap());

        // OK to delete things that don't exist.
        db.delete_vm_ids(&[VM_ID2, VM_ID3]).unwrap();

        assert_eq!(vec![VM_ID1], db.vm_ids_for_user(USER1).unwrap());
        assert_eq!(vec![VM_ID1], db.vm_ids_for_app(USER1, APP_A).unwrap());
        assert_eq!(1, db.count_vm_ids_for_app(USER1, APP_A).unwrap());

        db.add_vm_id(&VM_ID2, USER1, APP_A).unwrap();
        db.add_vm_id(&VM_ID3, USER1, APP_A).unwrap();

        assert_eq!(vec![VM_ID1, VM_ID2, VM_ID3], db.vm_ids_for_user(USER1).unwrap());
        assert_eq!(vec![VM_ID1, VM_ID2, VM_ID3], db.vm_ids_for_app(USER1, APP_A).unwrap());
        assert_eq!(3, db.count_vm_ids_for_app(USER1, APP_A).unwrap());
        assert_eq!(vec![VM_ID4], db.vm_ids_for_app(USER2, APP_B).unwrap());
        assert_eq!(1, db.count_vm_ids_for_app(USER2, APP_B).unwrap());
        assert_eq!(vec![VM_ID5], db.vm_ids_for_user(USER3).unwrap());
        assert_eq!(empty, db.vm_ids_for_user(USER_UNKNOWN).unwrap());
        assert_eq!(empty, db.vm_ids_for_app(USER1, APP_UNKNOWN).unwrap());
        assert_eq!(0, db.count_vm_ids_for_app(USER1, APP_UNKNOWN).unwrap());

        assert_eq!(
            vec![(USER1, APP_A), (USER2, APP_B), (USER3, APP_C)],
            db.get_all_owners().unwrap()
        );

        show_contents(&db);
    }

    #[test]
    fn test_invalid_vm_id() {
        let mut db = new_test_db();
        db.add_vm_id(&VM_ID3, USER1, APP_A).unwrap();
        db.add_vm_id(&VM_ID2, USER1, APP_A).unwrap();
        db.add_vm_id(&VM_ID1, USER1, APP_A).unwrap();

        // Note that results are returned in `vm_id` order, because the table is `WITHOUT ROWID`.
        assert_eq!(vec![VM_ID1, VM_ID2, VM_ID3], db.vm_ids_for_user(USER1).unwrap());

        // Manually insert a row with a VM ID that's the wrong size.
        db.conn
            .execute(
                "REPLACE INTO main.vmids (vm_id, user_id, app_id, created) VALUES (?1, ?2, ?3, ?4);",
                params![&[99u8; 60], &USER1, APP_A, &db_now()],
            )
            .unwrap();

        // Invalid row is skipped and remainder returned.
        assert_eq!(vec![VM_ID1, VM_ID2, VM_ID3], db.vm_ids_for_user(USER1).unwrap());
        show_contents(&db);
    }

    #[test]
    fn test_remove_oldest_with_upgrade() {
        let mut db = new_test_db_version(0);
        let version = db.schema_version().unwrap();
        assert_eq!(0, version);

        let remove_count = 10;
        let mut want = vec![];

        // Manually insert rows before upgrade.
        const V0_COUNT: usize = 5;
        for idx in 0..V0_COUNT {
            let mut vm_id = [0u8; 64];
            vm_id[0..8].copy_from_slice(&(idx as u64).to_be_bytes());
            if want.len() < remove_count {
                want.push(vm_id);
            }
            db.conn
                .execute(
                    "REPLACE INTO main.vmids (vm_id, user_id, app_id) VALUES (?1, ?2, ?3);",
                    params![&vm_id, &USER1, APP_A],
                )
                .unwrap();
        }

        // Now move to v1.
        db.upgrade_tables_v0_v1().unwrap();
        let version = db.schema_version().unwrap();
        assert_eq!(1, version);

        for idx in V0_COUNT..40 {
            let mut vm_id = [0u8; 64];
            vm_id[0..8].copy_from_slice(&(idx as u64).to_be_bytes());
            if want.len() < remove_count {
                want.push(vm_id);
            }
            db.add_vm_id(&vm_id, USER1, APP_A).unwrap();
        }
        show_contents_for_app(&db, USER1, APP_A, 10);
        let got = db.oldest_vm_ids_for_app(USER1, APP_A, 10).unwrap();
        assert_eq!(got, want);
    }
}
