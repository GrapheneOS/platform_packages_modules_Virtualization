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
use android_system_virtualizationmaintenance::aidl::android::system::virtualizationmaintenance;
use anyhow::{anyhow, Context, Result};
use binder::Strong;
use log::{error, info, warn};
use virtualizationmaintenance::IVirtualizationReconciliationCallback::IVirtualizationReconciliationCallback;

mod vmdb;
use vmdb::{VmId, VmIdDb};

/// Indicate whether an app ID belongs to a system core component.
fn core_app_id(app_id: i32) -> bool {
    app_id < 10000
}

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

    /// Record a new VM ID.  If there is an existing owner (user_id, app_id) for the VM ID,
    /// it will be replaced.
    pub fn add_id(&mut self, vm_id: &VmId, user_id: u32, app_id: u32) -> Result<()> {
        let user_id: i32 = user_id.try_into().context(format!("user_id {user_id} out of range"))?;
        let app_id: i32 = app_id.try_into().context(format!("app_id {app_id} out of range"))?;
        self.vm_id_db.add_vm_id(vm_id, user_id, app_id)
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

    /// Perform reconciliation to allow for possibly missed notifications of user or app removal.
    pub fn reconcile(
        &mut self,
        callback: &Strong<dyn IVirtualizationReconciliationCallback>,
    ) -> Result<()> {
        // First, retrieve all (user_id, app_id) pairs that own a VM.
        let owners = self.vm_id_db.get_all_owners().context("failed to retrieve owners from DB")?;
        if owners.is_empty() {
            info!("no VM owners, nothing to do");
            return Ok(());
        }

        // Look for absent users.
        let mut users: Vec<i32> = owners.iter().map(|(u, _a)| *u).collect();
        users.sort();
        users.dedup();
        let users_exist = callback
            .doUsersExist(&users)
            .context(format!("failed to determine if {} users exist", users.len()))?;
        if users_exist.len() != users.len() {
            error!("callback returned {} bools for {} inputs!", users_exist.len(), users.len());
            return Err(anyhow!("unexpected number of results from callback"));
        }

        for (user_id, present) in users.into_iter().zip(users_exist.into_iter()) {
            if present {
                // User is still present, but are all of the associated apps?
                let mut apps: Vec<i32> = owners
                    .iter()
                    .filter_map(|(u, a)| if *u == user_id { Some(*a) } else { None })
                    .collect();
                apps.sort();
                apps.dedup();

                let apps_exist = callback
                    .doAppsExist(user_id, &apps)
                    .context(format!("failed to check apps for user {user_id}"))?;
                if apps_exist.len() != apps.len() {
                    error!(
                        "callback returned {} bools for {} inputs!",
                        apps_exist.len(),
                        apps.len()
                    );
                    return Err(anyhow!("unexpected number of results from callback"));
                }

                let missing_apps: Vec<i32> = apps
                    .iter()
                    .zip(apps_exist.into_iter())
                    .filter_map(|(app_id, present)| if present { None } else { Some(*app_id) })
                    .collect();

                for app_id in missing_apps {
                    if core_app_id(app_id) {
                        info!("Skipping deletion for core app {app_id} for user {user_id}");
                        continue;
                    }
                    info!("App {app_id} for user {user_id} absent, deleting associated VM IDs");
                    if let Err(err) = self.delete_ids_for_app(user_id, app_id) {
                        error!("Failed to delete VM ID for user {user_id} app {app_id}: {err:?}");
                    }
                }
            } else {
                info!("user {user_id} no longer present, deleting associated VM IDs");
                if let Err(err) = self.delete_ids_for_user(user_id) {
                    error!("Failed to delete VM IDs for user {user_id} : {err:?}");
                }
            }
        }

        Ok(())
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
    use virtualizationmaintenance::IVirtualizationReconciliationCallback::{
        BnVirtualizationReconciliationCallback
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

    struct Reconciliation {
        gone_users: Vec<i32>,
        gone_apps: Vec<i32>,
    }

    impl IVirtualizationReconciliationCallback for Reconciliation {
        fn doUsersExist(&self, user_ids: &[i32]) -> binder::Result<Vec<bool>> {
            Ok(user_ids.iter().map(|user_id| !self.gone_users.contains(user_id)).collect())
        }
        fn doAppsExist(&self, _user_id: i32, app_ids: &[i32]) -> binder::Result<Vec<bool>> {
            Ok(app_ids.iter().map(|app_id| !self.gone_apps.contains(app_id)).collect())
        }
    }
    impl binder::Interface for Reconciliation {}

    const VM_ID1: VmId = [1u8; 64];
    const VM_ID2: VmId = [2u8; 64];
    const VM_ID3: VmId = [3u8; 64];
    const VM_ID4: VmId = [4u8; 64];
    const VM_ID5: VmId = [5u8; 64];

    const USER1: i32 = 1;
    const USER2: i32 = 2;
    const USER3: i32 = 3;
    const APP_A: i32 = 10050;
    const APP_B: i32 = 10060;
    const APP_C: i32 = 10070;
    const CORE_APP_A: i32 = 45;

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
        let history = Arc::new(Mutex::new(Vec::new()));
        let mut sk_state = new_test_state(history.clone(), 2);

        sk_state.vm_id_db.add_vm_id(&VM_ID1, USER1, APP_A).unwrap();
        sk_state.vm_id_db.add_vm_id(&VM_ID2, USER1, APP_A).unwrap();
        sk_state.vm_id_db.add_vm_id(&VM_ID3, USER2, APP_B).unwrap();
        sk_state.vm_id_db.add_vm_id(&VM_ID4, USER3, APP_A).unwrap();
        sk_state.vm_id_db.add_vm_id(&VM_ID5, USER3, APP_C).unwrap(); // Overwrites APP_A
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

    #[test]
    fn test_sk_state_reconcile() {
        let history = Arc::new(Mutex::new(Vec::new()));
        let mut sk_state = new_test_state(history.clone(), 20);

        sk_state.vm_id_db.add_vm_id(&VM_ID1, USER1, APP_A).unwrap();
        sk_state.vm_id_db.add_vm_id(&VM_ID2, USER1, APP_A).unwrap();
        sk_state.vm_id_db.add_vm_id(&VM_ID3, USER2, APP_B).unwrap();
        sk_state.vm_id_db.add_vm_id(&VM_ID4, USER2, CORE_APP_A).unwrap();
        sk_state.vm_id_db.add_vm_id(&VM_ID5, USER3, APP_C).unwrap();

        assert_eq!(vec![VM_ID1, VM_ID2], sk_state.vm_id_db.vm_ids_for_user(USER1).unwrap());
        assert_eq!(vec![VM_ID1, VM_ID2], sk_state.vm_id_db.vm_ids_for_app(USER1, APP_A).unwrap());
        assert_eq!(vec![VM_ID3], sk_state.vm_id_db.vm_ids_for_app(USER2, APP_B).unwrap());
        assert_eq!(vec![VM_ID5], sk_state.vm_id_db.vm_ids_for_user(USER3).unwrap());

        // Perform a reconciliation and pretend that USER1 and [CORE_APP_A, APP_B] are gone.
        let reconciliation =
            Reconciliation { gone_users: vec![USER1], gone_apps: vec![CORE_APP_A, APP_B] };
        let callback = BnVirtualizationReconciliationCallback::new_binder(
            reconciliation,
            binder::BinderFeatures::default(),
        );
        sk_state.reconcile(&callback).unwrap();

        let empty: Vec<VmId> = Vec::new();
        assert_eq!(empty, sk_state.vm_id_db.vm_ids_for_user(USER1).unwrap());
        assert_eq!(empty, sk_state.vm_id_db.vm_ids_for_app(USER1, APP_A).unwrap());
        // VM for core app stays even though it's reported as absent.
        assert_eq!(vec![VM_ID4], sk_state.vm_id_db.vm_ids_for_user(USER2).unwrap());
        assert_eq!(empty, sk_state.vm_id_db.vm_ids_for_app(USER2, APP_B).unwrap());
        assert_eq!(vec![VM_ID5], sk_state.vm_id_db.vm_ids_for_user(USER3).unwrap());
    }

    struct Irreconcilable;

    impl IVirtualizationReconciliationCallback for Irreconcilable {
        fn doUsersExist(&self, user_ids: &[i32]) -> binder::Result<Vec<bool>> {
            panic!("doUsersExist called with {user_ids:?}");
        }
        fn doAppsExist(&self, user_id: i32, app_ids: &[i32]) -> binder::Result<Vec<bool>> {
            panic!("doAppsExist called with {user_id:?}, {app_ids:?}");
        }
    }
    impl binder::Interface for Irreconcilable {}

    #[test]
    fn test_sk_state_reconcile_not_needed() {
        let history = Arc::new(Mutex::new(Vec::new()));
        let mut sk_state = new_test_state(history.clone(), 20);

        sk_state.vm_id_db.add_vm_id(&VM_ID1, USER1, APP_A).unwrap();
        sk_state.vm_id_db.add_vm_id(&VM_ID2, USER1, APP_A).unwrap();
        sk_state.vm_id_db.add_vm_id(&VM_ID3, USER2, APP_B).unwrap();
        sk_state.vm_id_db.add_vm_id(&VM_ID5, USER3, APP_C).unwrap();
        sk_state.delete_ids_for_user(USER1).unwrap();
        sk_state.delete_ids_for_user(USER2).unwrap();
        sk_state.delete_ids_for_user(USER3).unwrap();

        // No extant secrets, so reconciliation should not trigger the callback.
        let callback = BnVirtualizationReconciliationCallback::new_binder(
            Irreconcilable,
            binder::BinderFeatures::default(),
        );
        sk_state.reconcile(&callback).unwrap();
    }
}
