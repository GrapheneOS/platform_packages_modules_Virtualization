/*
 * Copyright (C) 2021 The Android Open Source Project
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *      http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */
use anyhow::{anyhow, bail, Result};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::io;
use std::os::unix::ffi::OsStrExt;

/// `InodeTable` is a table of `InodeData` indexed by `Inode`.
#[derive(Debug)]
pub struct InodeTable {
    table: Vec<InodeData>,
}

/// `Inode` is the handle (or index in the table) to `InodeData` which represents an inode.
pub type Inode = u64;

const INVALID: Inode = 0;
const ROOT: Inode = 1;

#[cfg(multi_tenant)]
const READ_MODE: u32 = libc::S_IRUSR | libc::S_IRGRP;
#[cfg(multi_tenant)]
const EXECUTE_MODE: u32 = libc::S_IXUSR | libc::S_IXGRP;

#[cfg(not(multi_tenant))]
const READ_MODE: u32 = libc::S_IRUSR;
#[cfg(not(multi_tenant))]
const EXECUTE_MODE: u32 = libc::S_IXUSR;

const DEFAULT_DIR_MODE: u32 = READ_MODE | EXECUTE_MODE;
// b/264668376 some files in APK don't have unix permissions specified. Default to 400
// otherwise those files won't be readable even by the owner.
const DEFAULT_FILE_MODE: u32 = READ_MODE;
const EXECUTABLE_FILE_MODE: u32 = DEFAULT_FILE_MODE | EXECUTE_MODE;

/// `InodeData` represents an inode which has metadata about a file or a directory
#[derive(Debug)]
pub struct InodeData {
    /// Size of the file that this inode represents. In case when the file is a directory, this
    // is zero.
    pub size: u64,
    /// unix mode of this inode. It may not have `S_IFDIR` and `S_IFREG` in case the original zip
    /// doesn't have the information in the external_attributes fields. To test if this inode
    /// is for a regular file or a directory, use `is_dir`.
    pub mode: u32,
    data: InodeDataData,
}

type ZipIndex = usize;

/// `InodeDataData` is the actual data (or a means to access the data) of the file or the directory
/// that an inode is representing. In case of a directory, this data is the hash table of the
/// directory entries. In case of a file, this data is the index of the file in `ZipArchive` which
/// can be used to retrieve `ZipFile` that provides access to the content of the file.
#[derive(Debug)]
enum InodeDataData {
    Directory(HashMap<CString, DirectoryEntry>),
    File(ZipIndex),
}

#[derive(Debug, Clone)]
pub struct DirectoryEntry {
    pub inode: Inode,
    pub kind: InodeKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum InodeKind {
    Directory,
    File,
}

impl InodeData {
    pub fn is_dir(&self) -> bool {
        matches!(&self.data, InodeDataData::Directory(_))
    }

    pub fn get_directory(&self) -> Option<&HashMap<CString, DirectoryEntry>> {
        match &self.data {
            InodeDataData::Directory(hash) => Some(hash),
            _ => None,
        }
    }

    pub fn get_zip_index(&self) -> Option<ZipIndex> {
        match &self.data {
            InodeDataData::File(zip_index) => Some(*zip_index),
            _ => None,
        }
    }

    // Below methods are used to construct the inode table when initializing the filesystem. Once
    // the initialization is done, these are not used because this is a read-only filesystem.

    fn new_dir(mode: u32) -> InodeData {
        InodeData { mode, size: 0, data: InodeDataData::Directory(HashMap::new()) }
    }

    fn new_file(zip_index: ZipIndex, mode: u32, zip_file: &zip::read::ZipFile) -> InodeData {
        InodeData { mode, size: zip_file.size(), data: InodeDataData::File(zip_index) }
    }

    fn add_to_directory(&mut self, name: CString, entry: DirectoryEntry) {
        match &mut self.data {
            InodeDataData::Directory(hashtable) => {
                let existing = hashtable.insert(name, entry);
                assert!(existing.is_none());
            }
            _ => {
                panic!("can't add a directory entry to a file inode");
            }
        }
    }
}

impl InodeTable {
    /// Gets `InodeData` at a specific index.
    pub fn get(&self, inode: Inode) -> Option<&InodeData> {
        match inode {
            INVALID => None,
            _ => self.table.get(inode as usize),
        }
    }

    fn get_mut(&mut self, inode: Inode) -> Option<&mut InodeData> {
        match inode {
            INVALID => None,
            _ => self.table.get_mut(inode as usize),
        }
    }

    fn put(&mut self, data: InodeData) -> Inode {
        let inode = self.table.len() as Inode;
        self.table.push(data);
        inode
    }

    /// Finds the inode number of a file named `name` in the `parent` inode. The `parent` inode
    /// must exist and be a directory.
    fn find(&self, parent: Inode, name: &CStr) -> Option<Inode> {
        let data = self.get(parent).unwrap();
        match data.get_directory().unwrap().get(name) {
            Some(DirectoryEntry { inode, .. }) => Some(*inode),
            _ => None,
        }
    }

    // Adds the inode `data` to the inode table and also links it to the `parent` inode as a file
    // named `name`. The `parent` inode must exist and be a directory.
    fn add(&mut self, parent: Inode, name: CString, data: InodeData) -> Inode {
        assert!(self.find(parent, &name).is_none());

        let kind = if data.is_dir() { InodeKind::Directory } else { InodeKind::File };
        // Add the inode to the table
        let inode = self.put(data);

        // ... and then register it to the directory of the parent inode
        self.get_mut(parent).unwrap().add_to_directory(name, DirectoryEntry { inode, kind });
        inode
    }

    /// Constructs `InodeTable` from a zip archive `archive`.
    pub fn from_zip<R: io::Read + io::Seek>(
        archive: &mut zip::ZipArchive<R>,
    ) -> Result<InodeTable> {
        let mut table = InodeTable { table: Vec::new() };

        // Add the inodes for the invalid and the root directory
        assert_eq!(INVALID, table.put(InodeData::new_dir(0)));
        assert_eq!(ROOT, table.put(InodeData::new_dir(DEFAULT_DIR_MODE)));

        // For each zip file in the archive, create an inode and add it to the table. If the file's
        // parent directories don't have corresponding inodes in the table, handle them too.
        for i in 0..archive.len() {
            let file = archive.by_index(i)?;
            let path = file
                .enclosed_name()
                .ok_or_else(|| anyhow!("{} is an invalid name", file.name()))?;
            // TODO(jiyong): normalize this (e.g. a/b/c/../d -> a/b/d). We can't use
            // fs::canonicalize as this is a non-existing path yet.

            let mut parent = ROOT;
            let mut iter = path.iter().peekable();

            let mut file_mode = DEFAULT_FILE_MODE;
            if path.starts_with("bin/") {
                // Allow files under bin to have execute permission, this enables payloads to bundle
                // additional binaries that they might want to execute.
                // An example of such binary is measure_io one used in the authfs performance tests.
                // More context available at b/265261525 and b/270955654.
                file_mode = EXECUTABLE_FILE_MODE;
            }

            while let Some(name) = iter.next() {
                // TODO(jiyong): remove this check by canonicalizing `path`
                if name == ".." {
                    bail!(".. is not allowed");
                }

                let is_leaf = iter.peek().is_none();
                let is_file = file.is_file() && is_leaf;

                // The happy path; the inode for `name` is already in the `parent` inode. Move on
                // to the next path element.
                let name = CString::new(name.as_bytes()).unwrap();
                if let Some(found) = table.find(parent, &name) {
                    parent = found;
                    // Update the mode if this is a directory leaf.
                    if !is_file && is_leaf {
                        let inode = table.get_mut(parent).unwrap();
                        inode.mode = file.unix_mode().unwrap_or(DEFAULT_DIR_MODE);
                    }
                    continue;
                }

                // No inode found. Create a new inode and add it to the inode table.
                // At the moment of writing this comment the apk file doesn't specify any
                // permissions (apart from the ones on lib/), but it might change in the future.
                // TODO(b/270955654): should we control the file permissions ourselves?
                let inode = if is_file {
                    InodeData::new_file(i, file.unix_mode().unwrap_or(file_mode), &file)
                } else if is_leaf {
                    InodeData::new_dir(file.unix_mode().unwrap_or(DEFAULT_DIR_MODE))
                } else {
                    InodeData::new_dir(DEFAULT_DIR_MODE)
                };
                let new = table.add(parent, name, inode);
                parent = new;
            }
        }
        Ok(table)
    }
}

#[cfg(test)]
mod tests {
    use crate::inode::*;
    use std::io::{Cursor, Write};
    use zip::write::FileOptions;

    // Creates an in-memory zip buffer, adds some files to it, and converts it to InodeTable
    fn setup(add: fn(&mut zip::ZipWriter<&mut std::io::Cursor<Vec<u8>>>)) -> InodeTable {
        let mut buf: Cursor<Vec<u8>> = Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(&mut buf);
        add(&mut writer);
        assert!(writer.finish().is_ok());
        drop(writer);

        let zip = zip::ZipArchive::new(buf);
        assert!(zip.is_ok());
        let it = InodeTable::from_zip(&mut zip.unwrap());
        assert!(it.is_ok());
        it.unwrap()
    }

    fn check_dir(it: &InodeTable, parent: Inode, name: &str) -> Inode {
        let name = CString::new(name.as_bytes()).unwrap();
        let inode = it.find(parent, &name);
        assert!(inode.is_some());
        let inode = inode.unwrap();
        let inode_data = it.get(inode);
        assert!(inode_data.is_some());
        let inode_data = inode_data.unwrap();
        assert_eq!(0, inode_data.size);
        assert!(inode_data.is_dir());
        inode
    }

    fn check_file<'a>(it: &'a InodeTable, parent: Inode, name: &str) -> &'a InodeData {
        let name = CString::new(name.as_bytes()).unwrap();
        let inode = it.find(parent, &name);
        assert!(inode.is_some());
        let inode = inode.unwrap();
        let inode_data = it.get(inode);
        assert!(inode_data.is_some());
        let inode_data = inode_data.unwrap();
        assert!(!inode_data.is_dir());
        inode_data
    }

    #[test]
    fn empty_zip_has_two_inodes() {
        let it = setup(|_| {});
        assert_eq!(2, it.table.len());
        assert!(it.get(INVALID).is_none());
        assert!(it.get(ROOT).is_some());
    }

    #[test]
    fn one_file() {
        let it = setup(|zip| {
            zip.start_file("foo", FileOptions::default()).unwrap();
            zip.write_all(b"0123456789").unwrap();
        });
        let inode_data = check_file(&it, ROOT, "foo");
        assert_eq!(b"0123456789".len() as u64, inode_data.size);
    }

    #[test]
    fn one_dir() {
        let it = setup(|zip| {
            zip.add_directory("foo", FileOptions::default()).unwrap();
        });
        let inode = check_dir(&it, ROOT, "foo");
        // The directory doesn't have any entries
        assert_eq!(0, it.get(inode).unwrap().get_directory().unwrap().len());
    }

    #[test]
    fn one_file_in_subdirs() {
        let it = setup(|zip| {
            zip.start_file("a/b/c/d", FileOptions::default()).unwrap();
            zip.write_all(b"0123456789").unwrap();
        });

        assert_eq!(6, it.table.len());
        let a = check_dir(&it, ROOT, "a");
        let b = check_dir(&it, a, "b");
        let c = check_dir(&it, b, "c");
        let d = check_file(&it, c, "d");
        assert_eq!(10, d.size);
    }

    #[test]
    fn complex_hierarchy() {
        // root/
        //   a/
        //    b1/
        //    b2/
        //      c1 (file)
        //      c2/
        //          d1 (file)
        //          d2 (file)
        //          d3 (file)
        //  x/
        //    y1 (file)
        //    y2 (file)
        //    y3/
        //
        //  foo (file)
        //  bar (file)
        let it = setup(|zip| {
            let opt = FileOptions::default();
            zip.add_directory("a/b1", opt).unwrap();

            zip.start_file("a/b2/c1", opt).unwrap();

            zip.start_file("a/b2/c2/d1", opt).unwrap();
            zip.start_file("a/b2/c2/d2", opt).unwrap();
            zip.start_file("a/b2/c2/d3", opt).unwrap();

            zip.start_file("x/y1", opt).unwrap();
            zip.start_file("x/y2", opt).unwrap();
            zip.add_directory("x/y3", opt).unwrap();

            zip.start_file("foo", opt).unwrap();
            zip.start_file("bar", opt).unwrap();
        });

        assert_eq!(16, it.table.len()); // 8 files, 6 dirs, and 2 (for root and the invalid inode)
        let a = check_dir(&it, ROOT, "a");
        let _b1 = check_dir(&it, a, "b1");
        let b2 = check_dir(&it, a, "b2");
        let _c1 = check_file(&it, b2, "c1");

        let c2 = check_dir(&it, b2, "c2");
        let _d1 = check_file(&it, c2, "d1");
        let _d2 = check_file(&it, c2, "d3");
        let _d3 = check_file(&it, c2, "d3");

        let x = check_dir(&it, ROOT, "x");
        let _y1 = check_file(&it, x, "y1");
        let _y2 = check_file(&it, x, "y2");
        let _y3 = check_dir(&it, x, "y3");

        let _foo = check_file(&it, ROOT, "foo");
        let _bar = check_file(&it, ROOT, "bar");
    }

    #[test]
    fn file_size() {
        let it = setup(|zip| {
            let opt = FileOptions::default();
            zip.start_file("empty", opt).unwrap();

            zip.start_file("10bytes", opt).unwrap();
            zip.write_all(&[0; 10]).unwrap();

            zip.start_file("1234bytes", opt).unwrap();
            zip.write_all(&[0; 1234]).unwrap();

            zip.start_file("2^20bytes", opt).unwrap();
            zip.write_all(&[0; 2 << 20]).unwrap();
        });

        let f = check_file(&it, ROOT, "empty");
        assert_eq!(0, f.size);

        let f = check_file(&it, ROOT, "10bytes");
        assert_eq!(10, f.size);

        let f = check_file(&it, ROOT, "1234bytes");
        assert_eq!(1234, f.size);

        let f = check_file(&it, ROOT, "2^20bytes");
        assert_eq!(2 << 20, f.size);
    }

    #[test]
    fn rejects_invalid_paths() {
        let invalid_paths = [
            "a/../../b",   // escapes the root
            "a/..",        // escapes the root
            "a/../../b/c", // escape the root
            "a/b/../c",    // doesn't escape the root, but not normalized
        ];
        for path in invalid_paths.iter() {
            let mut buf: Cursor<Vec<u8>> = Cursor::new(Vec::new());
            let mut writer = zip::ZipWriter::new(&mut buf);
            writer.start_file(*path, FileOptions::default()).unwrap();
            assert!(writer.finish().is_ok());
            drop(writer);

            let zip = zip::ZipArchive::new(buf);
            assert!(zip.is_ok());
            let it = InodeTable::from_zip(&mut zip.unwrap());
            assert!(it.is_err());
        }
    }
}
