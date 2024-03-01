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

use anyhow::{Context, Result};
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

/// Identifier for a VM and its corresponding secret.
pub type VmId = [u8; 64];

/// Representation of an on-disk database of VM IDs.
pub struct VmIdDb {
    conn: Connection,
}

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
        let mut result = Self {
            conn: Connection::open_with_flags(db_path, flags)
                .context(format!("failed to open/create DB with {flags:?}"))?,
        };

        if created {
            result.init_tables().context("failed to create tables")?;
        }
        Ok((result, created))
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

    /// Create the database table and indices.
    fn init_tables(&mut self) -> Result<()> {
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
    #[allow(dead_code)] // TODO(b/294177871): connect this up
    pub fn add_vm_id(&mut self, vm_id: &VmId, user_id: i32, app_id: i32) -> Result<()> {
        let _rows = self
            .conn
            .execute(
                "REPLACE INTO main.vmids (vm_id, user_id, app_id) VALUES (?1, ?2, ?3);",
                params![vm_id, &user_id, &app_id],
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
                Err(e) => log::error!("failed to parse row: {e:?}"),
            }
        }

        Ok(vm_ids)
    }
}

#[cfg(test)]
pub fn new_test_db() -> VmIdDb {
    let mut db = VmIdDb { conn: Connection::open_in_memory().unwrap() };
    db.init_tables().unwrap();
    db
}

#[cfg(test)]
mod tests {
    use super::*;
    const VM_ID1: VmId = [1u8; 64];
    const VM_ID2: VmId = [2u8; 64];
    const VM_ID3: VmId = [3u8; 64];
    const VM_ID4: VmId = [4u8; 64];
    const VM_ID5: VmId = [5u8; 64];
    const USER1: i32 = 1;
    const USER2: i32 = 2;
    const USER3: i32 = 3;
    const USER_UNKNOWN: i32 = 4;
    const APP_A: i32 = 50;
    const APP_B: i32 = 60;
    const APP_C: i32 = 70;
    const APP_UNKNOWN: i32 = 99;

    #[test]
    fn test_add_remove() {
        let mut db = new_test_db();
        db.add_vm_id(&VM_ID1, USER1, APP_A).unwrap();
        db.add_vm_id(&VM_ID2, USER1, APP_A).unwrap();
        db.add_vm_id(&VM_ID3, USER1, APP_A).unwrap();
        db.add_vm_id(&VM_ID4, USER2, APP_B).unwrap();
        db.add_vm_id(&VM_ID5, USER3, APP_A).unwrap();
        db.add_vm_id(&VM_ID5, USER3, APP_C).unwrap();
        let empty: Vec<VmId> = Vec::new();

        assert_eq!(vec![VM_ID1, VM_ID2, VM_ID3], db.vm_ids_for_user(USER1).unwrap());
        assert_eq!(vec![VM_ID1, VM_ID2, VM_ID3], db.vm_ids_for_app(USER1, APP_A).unwrap());
        assert_eq!(vec![VM_ID4], db.vm_ids_for_app(USER2, APP_B).unwrap());
        assert_eq!(vec![VM_ID5], db.vm_ids_for_user(USER3).unwrap());
        assert_eq!(empty, db.vm_ids_for_user(USER_UNKNOWN).unwrap());
        assert_eq!(empty, db.vm_ids_for_app(USER1, APP_UNKNOWN).unwrap());

        db.delete_vm_ids(&[VM_ID2, VM_ID3]).unwrap();

        assert_eq!(vec![VM_ID1], db.vm_ids_for_user(USER1).unwrap());
        assert_eq!(vec![VM_ID1], db.vm_ids_for_app(USER1, APP_A).unwrap());

        // OK to delete things that don't exist.
        db.delete_vm_ids(&[VM_ID2, VM_ID3]).unwrap();

        assert_eq!(vec![VM_ID1], db.vm_ids_for_user(USER1).unwrap());
        assert_eq!(vec![VM_ID1], db.vm_ids_for_app(USER1, APP_A).unwrap());

        db.add_vm_id(&VM_ID2, USER1, APP_A).unwrap();
        db.add_vm_id(&VM_ID3, USER1, APP_A).unwrap();

        assert_eq!(vec![VM_ID1, VM_ID2, VM_ID3], db.vm_ids_for_user(USER1).unwrap());
        assert_eq!(vec![VM_ID1, VM_ID2, VM_ID3], db.vm_ids_for_app(USER1, APP_A).unwrap());
        assert_eq!(vec![VM_ID4], db.vm_ids_for_app(USER2, APP_B).unwrap());
        assert_eq!(vec![VM_ID5], db.vm_ids_for_user(USER3).unwrap());
        assert_eq!(empty, db.vm_ids_for_user(USER_UNKNOWN).unwrap());
        assert_eq!(empty, db.vm_ids_for_app(USER1, APP_UNKNOWN).unwrap());
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
                "REPLACE INTO main.vmids (vm_id, user_id, app_id) VALUES (?1, ?2, ?3);",
                params![&[99u8; 60], &USER1, APP_A],
            )
            .unwrap();

        // Invalid row is skipped and remainder returned.
        assert_eq!(vec![VM_ID1, VM_ID2, VM_ID3], db.vm_ids_for_user(USER1).unwrap());
    }
}
