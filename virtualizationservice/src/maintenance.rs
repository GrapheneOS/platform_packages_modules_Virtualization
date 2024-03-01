// Copyright 2024 The Android Open Source Project
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use android_hardware_security_secretkeeper::aidl::android::hardware::security::secretkeeper::{
    ISecretkeeper::ISecretkeeper, SecretId::SecretId,
};
use anyhow::Result;
use log::{error, info, warn};

mod vmdb;
use vmdb::{VmId, VmIdDb};

/// Interface name for the Secretkeeper HAL.
const SECRETKEEPER_SERVICE: &str = "android.hardware.security.secretkeeper.ISecretkeeper/default";

/// Directory in which to write persistent state.
const PERSISTENT_DIRECTORY: &str = "/data/misc/apexdata/com.android.virt";

/// Maximum number of VM IDs to delete at once.  Needs to be smaller than both the maximum
/// number of SQLite parameters (999) and also small enough that an ISecretkeeper::deleteIds
/// parcel fits within max AIDL message size.
const DELETE_MAX_BATCH_SIZE: usize = 100;

/// State related to VM secrets.
pub struct State {
    sk: binder::Strong<dyn ISecretkeeper>,
    /// Database of VM IDs,
    vm_id_db: VmIdDb,
    batch_size: usize,
}

impl State {
    pub fn new() -> Option<Self> {
        let sk = match Self::find_sk() {
            Some(sk) => sk,
            None => {
                warn!("failed to find a Secretkeeper instance; skipping secret management");
                return None;
            }
        };
        let (vm_id_db, created) = match VmIdDb::new(PERSISTENT_DIRECTORY) {
            Ok(v) => v,
            Err(e) => {
                error!("skipping secret management, failed to connect to database: {e:?}");
                return None;
            }
        };
        if created {
            // If the database did not previously exist, then this appears to be the first run of
            // `virtualizationservice` since device setup or factory reset.  In case of the latter,
            // delete any secrets that may be left over from before reset, thus ensuring that the
            // local database state matches that of the TA (i.e. empty).
            warn!("no existing VM ID DB; clearing any previous secrets to match fresh DB");
            if let Err(e) = sk.deleteAll() {
                error!("failed to delete previous secrets, dropping database: {e:?}");
                vm_id_db.delete_db_file(PERSISTENT_DIRECTORY);
                return None;
            }
        } else {
            info!("re-using existing VM ID DB");
        }
        Some(Self { sk, vm_id_db, batch_size: DELETE_MAX_BATCH_SIZE })
    }

    fn find_sk() -> Option<binder::Strong<dyn ISecretkeeper>> {
        if let Ok(true) = binder::is_declared(SECRETKEEPER_SERVICE) {
            match binder::get_interface(SECRETKEEPER_SERVICE) {
                Ok(sk) => Some(sk),
                Err(e) => {
                    error!("failed to connect to {SECRETKEEPER_SERVICE}: {e:?}");
                    None
                }
            }
        } else {
            info!("instance {SECRETKEEPER_SERVICE} not declared");
            None
        }
    }

    /// Delete the VM IDs associated with Android user ID `user_id`.
    pub fn delete_ids_for_user(&mut self, user_id: i32) -> Result<()> {
        let vm_ids = self.vm_id_db.vm_ids_for_user(user_id)?;
        info!(
            "delete_ids_for_user(user_id={user_id}) triggers deletion of {} secrets",
            vm_ids.len()
        );
        self.delete_ids(&vm_ids);
        Ok(())
    }

    /// Delete the VM IDs associated with `(user_id, app_id)`.
    pub fn delete_ids_for_app(&mut self, user_id: i32, app_id: i32) -> Result<()> {
        let vm_ids = self.vm_id_db.vm_ids_for_app(user_id, app_id)?;
        info!(
            "delete_ids_for_app(user_id={user_id}, app_id={app_id}) removes {} secrets",
            vm_ids.len()
        );
        self.delete_ids(&vm_ids);
        Ok(())
    }

    /// Delete the provided VM IDs from both Secretkeeper and the database.
    pub fn delete_ids(&mut self, mut vm_ids: &[VmId]) {
        while !vm_ids.is_empty() {
            let len = std::cmp::min(vm_ids.len(), self.batch_size);
            let batch = &vm_ids[..len];
            self.delete_ids_batch(batch);
            vm_ids = &vm_ids[len..];
        }
    }

    /// Delete a batch of VM IDs from both Secretkeeper and the database. The batch is assumed
    /// to be smaller than both:
    /// - the corresponding limit for number of database parameters
    /// - the corresponding limit for maximum size of a single AIDL message for `ISecretkeeper`.
    fn delete_ids_batch(&mut self, vm_ids: &[VmId]) {
        let secret_ids: Vec<SecretId> = vm_ids.iter().map(|id| SecretId { id: *id }).collect();
        if let Err(e) = self.sk.deleteIds(&secret_ids) {
            error!("failed to delete all secrets from Secretkeeper: {e:?}");
        }
        if let Err(e) = self.vm_id_db.delete_vm_ids(vm_ids) {
            error!("failed to remove secret IDs from database: {e:?}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use android_hardware_security_authgraph::aidl::android::hardware::security::authgraph::{
        IAuthGraphKeyExchange::IAuthGraphKeyExchange,
    };
    use android_hardware_security_secretkeeper::aidl::android::hardware::security::secretkeeper::{
        ISecretkeeper::BnSecretkeeper
    };

    /// Fake implementation of Secretkeeper that keeps a history of what operations were invoked.
    #[derive(Default)]
    struct FakeSk {
        history: Arc<Mutex<Vec<SkOp>>>,
    }

    #[derive(Clone, PartialEq, Eq, Debug)]
    enum SkOp {
        Management,
        DeleteIds(Vec<VmId>),
        DeleteAll,
    }

    impl ISecretkeeper for FakeSk {
        fn processSecretManagementRequest(&self, _req: &[u8]) -> binder::Result<Vec<u8>> {
            self.history.lock().unwrap().push(SkOp::Management);
            Ok(vec![])
        }

        fn getAuthGraphKe(&self) -> binder::Result<binder::Strong<dyn IAuthGraphKeyExchange>> {
            unimplemented!()
        }

        fn deleteIds(&self, ids: &[SecretId]) -> binder::Result<()> {
            self.history.lock().unwrap().push(SkOp::DeleteIds(ids.iter().map(|s| s.id).collect()));
            Ok(())
        }

        fn deleteAll(&self) -> binder::Result<()> {
            self.history.lock().unwrap().push(SkOp::DeleteAll);
            Ok(())
        }
    }
    impl binder::Interface for FakeSk {}

    fn new_test_state(history: Arc<Mutex<Vec<SkOp>>>, batch_size: usize) -> State {
        let vm_id_db = vmdb::new_test_db();
        let sk = FakeSk { history };
        let sk = BnSecretkeeper::new_binder(sk, binder::BinderFeatures::default());
        State { sk, vm_id_db, batch_size }
    }

    const VM_ID1: VmId = [1u8; 64];
    const VM_ID2: VmId = [2u8; 64];
    const VM_ID3: VmId = [3u8; 64];
    const VM_ID4: VmId = [4u8; 64];
    const VM_ID5: VmId = [5u8; 64];

    #[test]
    fn test_sk_state_batching() {
        let history = Arc::new(Mutex::new(Vec::new()));
        let mut sk_state = new_test_state(history.clone(), 2);
        sk_state.delete_ids(&[VM_ID1, VM_ID2, VM_ID3, VM_ID4, VM_ID5]);
        let got = (*history.lock().unwrap()).clone();
        assert_eq!(
            got,
            vec![
                SkOp::DeleteIds(vec![VM_ID1, VM_ID2]),
                SkOp::DeleteIds(vec![VM_ID3, VM_ID4]),
                SkOp::DeleteIds(vec![VM_ID5]),
            ]
        );
    }

    #[test]
    fn test_sk_state_no_batching() {
        let history = Arc::new(Mutex::new(Vec::new()));
        let mut sk_state = new_test_state(history.clone(), 6);
        sk_state.delete_ids(&[VM_ID1, VM_ID2, VM_ID3, VM_ID4, VM_ID5]);
        let got = (*history.lock().unwrap()).clone();
        assert_eq!(got, vec![SkOp::DeleteIds(vec![VM_ID1, VM_ID2, VM_ID3, VM_ID4, VM_ID5])]);
    }

    #[test]
    fn test_sk_state() {
        const USER1: i32 = 1;
        const USER2: i32 = 2;
        const USER3: i32 = 3;
        const APP_A: i32 = 50;
        const APP_B: i32 = 60;
        const APP_C: i32 = 70;

        let history = Arc::new(Mutex::new(Vec::new()));
        let mut sk_state = new_test_state(history.clone(), 2);

        sk_state.vm_id_db.add_vm_id(&VM_ID1, USER1, APP_A).unwrap();
        sk_state.vm_id_db.add_vm_id(&VM_ID2, USER1, APP_A).unwrap();
        sk_state.vm_id_db.add_vm_id(&VM_ID3, USER2, APP_B).unwrap();
        sk_state.vm_id_db.add_vm_id(&VM_ID4, USER3, APP_A).unwrap();
        sk_state.vm_id_db.add_vm_id(&VM_ID5, USER3, APP_C).unwrap();
        assert_eq!((*history.lock().unwrap()).clone(), vec![]);

        sk_state.delete_ids_for_app(USER2, APP_B).unwrap();
        assert_eq!((*history.lock().unwrap()).clone(), vec![SkOp::DeleteIds(vec![VM_ID3])]);

        sk_state.delete_ids_for_user(USER3).unwrap();
        assert_eq!(
            (*history.lock().unwrap()).clone(),
            vec![SkOp::DeleteIds(vec![VM_ID3]), SkOp::DeleteIds(vec![VM_ID4, VM_ID5]),]
        );

        assert_eq!(vec![VM_ID1, VM_ID2], sk_state.vm_id_db.vm_ids_for_user(USER1).unwrap());
        assert_eq!(vec![VM_ID1, VM_ID2], sk_state.vm_id_db.vm_ids_for_app(USER1, APP_A).unwrap());
        let empty: Vec<VmId> = Vec::new();
        assert_eq!(empty, sk_state.vm_id_db.vm_ids_for_app(USER2, APP_B).unwrap());
        assert_eq!(empty, sk_state.vm_id_db.vm_ids_for_user(USER3).unwrap());
    }
}
