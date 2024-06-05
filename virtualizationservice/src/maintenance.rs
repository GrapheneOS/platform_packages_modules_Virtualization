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

/// Interface name for the Secretkeeper HAL.
const SECRETKEEPER_SERVICE: &str = "android.hardware.security.secretkeeper.ISecretkeeper/default";

/// Directory in which to write persistent state.
const PERSISTENT_DIRECTORY: &str = "/data/misc/apexdata/com.android.virt";

/// Maximum number of VM IDs to delete at once.  Needs to be smaller than both the maximum
/// number of SQLite parameters (999) and also small enough that an ISecretkeeper::deleteIds
/// parcel fits within max AIDL message size.
const DELETE_MAX_BATCH_SIZE: usize = 100;

/// Maximum number of VM IDs that a single app can have.
const MAX_VM_IDS_PER_APP: usize = 400;

/// State related to VM secrets.
pub struct State {
    /// The real state, lazily created when we first need it.
    inner: Option<InnerState>,
}

struct InnerState {
    sk: binder::Strong<dyn ISecretkeeper>,
    /// Database of VM IDs,
    vm_id_db: VmIdDb,
    batch_size: usize,
}

impl State {
    pub fn new() -> Option<Self> {
        if is_sk_present() {
            // Don't instantiate the inner state yet, that will happen when it is needed.
            Some(Self { inner: None })
        } else {
            // If the Secretkeeper HAL doesn't exist, there's never any point in trying to
            // handle maintenance for it.
            info!("Failed to find a Secretkeeper instance; skipping secret management");
            None
        }
    }

    /// Return the existing inner state, or create one if there isn't one.
    /// This is done on demand as in early boot (before we need Secretkeeper) it may not be
    /// available to connect to. See b/331417880.
    fn get_inner(&mut self) -> Result<&mut InnerState> {
        if self.inner.is_none() {
            self.inner = Some(InnerState::new()?);
        }
        Ok(self.inner.as_mut().unwrap())
    }

    /// Record a new VM ID.  If there is an existing owner (user_id, app_id) for the VM ID,
    /// it will be replaced.
    pub fn add_id(&mut self, vm_id: &VmId, user_id: u32, app_id: u32) -> Result<()> {
        self.get_inner()?.add_id(vm_id, user_id, app_id)
    }

    /// Delete the VM IDs associated with Android user ID `user_id`.
    pub fn delete_ids_for_user(&mut self, user_id: i32) -> Result<()> {
        self.get_inner()?.delete_ids_for_user(user_id)
    }

    /// Delete the VM IDs associated with `(user_id, app_id)`.
    pub fn delete_ids_for_app(&mut self, user_id: i32, app_id: i32) -> Result<()> {
        self.get_inner()?.delete_ids_for_app(user_id, app_id)
    }

    /// Delete the provided VM ID associated with `(user_id, app_id)` from both Secretkeeper and
    /// the database.
    pub fn delete_id(&mut self, vm_id: &VmId, user_id: u32, app_id: u32) {
        let Ok(inner) = self.get_inner() else {
            warn!("No Secretkeeper available, not deleting secrets");
            return;
        };

        inner.delete_id_for_app(vm_id, user_id, app_id)
    }

    /// Perform reconciliation to allow for possibly missed notifications of user or app removal.
    pub fn reconcile(
        &mut self,
        callback: &Strong<dyn IVirtualizationReconciliationCallback>,
    ) -> Result<()> {
        self.get_inner()?.reconcile(callback)
    }
}

impl InnerState {
    fn new() -> Result<Self> {
        info!("Connecting to {SECRETKEEPER_SERVICE}");
        let sk = binder::wait_for_interface::<dyn ISecretkeeper>(SECRETKEEPER_SERVICE)
            .context("Connecting to {SECRETKEEPER_SERVICE}")?;
        let (vm_id_db, created) = VmIdDb::new(PERSISTENT_DIRECTORY)
            .context("Connecting to secret management database")?;
        if created {
            // If the database did not previously exist, then this appears to be the first run of
            // `virtualizationservice` since device setup or factory reset.  In case of the latter,
            // delete any secrets that may be left over from before reset, thus ensuring that the
            // local database state matches that of the TA (i.e. empty).
            warn!("no existing VM ID DB; clearing any previous secrets to match fresh DB");
            if let Err(e) = sk.deleteAll() {
                error!("failed to delete previous secrets, dropping database: {e:?}");
                vm_id_db.delete_db_file(PERSISTENT_DIRECTORY);
                return Err(e.into());
            }
        } else {
            info!("re-using existing VM ID DB");
        }
        Ok(Self { sk, vm_id_db, batch_size: DELETE_MAX_BATCH_SIZE })
    }

    fn add_id(&mut self, vm_id: &VmId, user_id: u32, app_id: u32) -> Result<()> {
        let user_id: i32 = user_id.try_into().context(format!("user_id {user_id} out of range"))?;
        let app_id: i32 = app_id.try_into().context(format!("app_id {app_id} out of range"))?;

        // To prevent unbounded growth of VM IDs (and the associated state) for an app, limit the
        // number of VM IDs per app.
        let count = self
            .vm_id_db
            .count_vm_ids_for_app(user_id, app_id)
            .context("failed to determine VM count")?;
        if count >= MAX_VM_IDS_PER_APP {
            // The owner has too many VM IDs, so delete the oldest IDs so that the new VM ID
            // creation can progress/succeed.
            let purge = 1 + count - MAX_VM_IDS_PER_APP;
            let old_vm_ids = self
                .vm_id_db
                .oldest_vm_ids_for_app(user_id, app_id, purge)
                .context("failed to find oldest VM IDs")?;
            error!("Deleting {purge} of {count} VM IDs for user_id={user_id}, app_id={app_id}");
            self.delete_ids(&old_vm_ids);
        }
        self.vm_id_db.add_vm_id(vm_id, user_id, app_id)
    }

    fn delete_id_for_app(&mut self, vm_id: &VmId, user_id: u32, app_id: u32) {
        if !self.vm_id_db.is_vm_id_for_app(vm_id, user_id, app_id).unwrap_or(false) {
            info!(
                "delete_id_for_app - VM id not associated with user_id={user_id}, app_id={app_id}"
            );
            return;
        }
        self.delete_ids(&[*vm_id])
    }

    fn delete_ids_for_user(&mut self, user_id: i32) -> Result<()> {
        let vm_ids = self.vm_id_db.vm_ids_for_user(user_id)?;
        info!(
            "delete_ids_for_user(user_id={user_id}) triggers deletion of {} secrets",
            vm_ids.len()
        );
        self.delete_ids(&vm_ids);
        Ok(())
    }

    fn delete_ids_for_app(&mut self, user_id: i32, app_id: i32) -> Result<()> {
        let vm_ids = self.vm_id_db.vm_ids_for_app(user_id, app_id)?;
        info!(
            "delete_ids_for_app(user_id={user_id}, app_id={app_id}) removes {} secrets",
            vm_ids.len()
        );
        self.delete_ids(&vm_ids);
        Ok(())
    }

    fn delete_ids(&mut self, mut vm_ids: &[VmId]) {
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

    fn reconcile(
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

/// Indicate whether an app ID belongs to a system core component.
fn core_app_id(app_id: i32) -> bool {
    app_id < 10000
}

fn is_sk_present() -> bool {
    matches!(binder::is_declared(SECRETKEEPER_SERVICE), Ok(true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use android_hardware_security_authgraph::aidl::android::hardware::security::authgraph;
    use android_hardware_security_secretkeeper::aidl::android::hardware::security::secretkeeper;
    use authgraph::IAuthGraphKeyExchange::IAuthGraphKeyExchange;
    use secretkeeper::ISecretkeeper::BnSecretkeeper;
    use std::sync::{Arc, Mutex};
    use virtualizationmaintenance::IVirtualizationReconciliationCallback::BnVirtualizationReconciliationCallback;

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
        let inner = InnerState { sk, vm_id_db, batch_size };
        State { inner: Some(inner) }
    }

    fn get_db(state: &mut State) -> &mut VmIdDb {
        &mut state.inner.as_mut().unwrap().vm_id_db
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
        let sk_state = new_test_state(history.clone(), 2);
        sk_state.inner.unwrap().delete_ids(&[VM_ID1, VM_ID2, VM_ID3, VM_ID4, VM_ID5]);
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
        let sk_state = new_test_state(history.clone(), 6);
        sk_state.inner.unwrap().delete_ids(&[VM_ID1, VM_ID2, VM_ID3, VM_ID4, VM_ID5]);
        let got = (*history.lock().unwrap()).clone();
        assert_eq!(got, vec![SkOp::DeleteIds(vec![VM_ID1, VM_ID2, VM_ID3, VM_ID4, VM_ID5])]);
    }

    #[test]
    fn test_sk_state() {
        let history = Arc::new(Mutex::new(Vec::new()));
        let mut sk_state = new_test_state(history.clone(), 2);

        get_db(&mut sk_state).add_vm_id(&VM_ID1, USER1, APP_A).unwrap();
        get_db(&mut sk_state).add_vm_id(&VM_ID2, USER1, APP_A).unwrap();
        get_db(&mut sk_state).add_vm_id(&VM_ID3, USER2, APP_B).unwrap();
        get_db(&mut sk_state).add_vm_id(&VM_ID4, USER3, APP_A).unwrap();
        get_db(&mut sk_state).add_vm_id(&VM_ID5, USER3, APP_C).unwrap();
        assert_eq!((*history.lock().unwrap()).clone(), vec![]);

        sk_state.delete_ids_for_app(USER2, APP_B).unwrap();
        assert_eq!((*history.lock().unwrap()).clone(), vec![SkOp::DeleteIds(vec![VM_ID3])]);

        sk_state.delete_ids_for_user(USER3).unwrap();
        assert_eq!(
            (*history.lock().unwrap()).clone(),
            vec![SkOp::DeleteIds(vec![VM_ID3]), SkOp::DeleteIds(vec![VM_ID4, VM_ID5]),]
        );

        assert_eq!(vec![VM_ID1, VM_ID2], get_db(&mut sk_state).vm_ids_for_user(USER1).unwrap());
        assert_eq!(
            vec![VM_ID1, VM_ID2],
            get_db(&mut sk_state).vm_ids_for_app(USER1, APP_A).unwrap()
        );
        let empty: Vec<VmId> = Vec::new();
        assert_eq!(empty, get_db(&mut sk_state).vm_ids_for_app(USER2, APP_B).unwrap());
        assert_eq!(empty, get_db(&mut sk_state).vm_ids_for_user(USER3).unwrap());
    }

    #[test]
    fn test_sk_state_delete_id() {
        let history = Arc::new(Mutex::new(Vec::new()));
        let mut sk_state = new_test_state(history.clone(), 2);

        get_db(&mut sk_state).add_vm_id(&VM_ID1, USER1, APP_A).unwrap();
        get_db(&mut sk_state).add_vm_id(&VM_ID2, USER1, APP_A).unwrap();
        get_db(&mut sk_state).add_vm_id(&VM_ID3, USER2, APP_B).unwrap();
        assert_eq!((*history.lock().unwrap()).clone(), vec![]);

        // A VM ID that doesn't exist anywhere - no delete
        sk_state.delete_id(&VM_ID4, USER1 as u32, APP_A as u32);
        assert_eq!((*history.lock().unwrap()).clone(), vec![]);

        // Wrong app ID - no delete
        sk_state.delete_id(&VM_ID1, USER1 as u32, APP_B as u32);
        assert_eq!((*history.lock().unwrap()).clone(), vec![]);

        // Wrong user ID - no delete
        sk_state.delete_id(&VM_ID1, USER2 as u32, APP_A as u32);
        assert_eq!((*history.lock().unwrap()).clone(), vec![]);

        // This porridge is just right.
        sk_state.delete_id(&VM_ID1, USER1 as u32, APP_A as u32);
        assert_eq!((*history.lock().unwrap()).clone(), vec![SkOp::DeleteIds(vec![VM_ID1])]);

        assert_eq!(vec![VM_ID2], get_db(&mut sk_state).vm_ids_for_user(USER1).unwrap());
        assert_eq!(vec![VM_ID3], get_db(&mut sk_state).vm_ids_for_user(USER2).unwrap());
    }

    #[test]
    fn test_sk_state_reconcile() {
        let history = Arc::new(Mutex::new(Vec::new()));
        let mut sk_state = new_test_state(history.clone(), 20);

        get_db(&mut sk_state).add_vm_id(&VM_ID1, USER1, APP_A).unwrap();
        get_db(&mut sk_state).add_vm_id(&VM_ID2, USER1, APP_A).unwrap();
        get_db(&mut sk_state).add_vm_id(&VM_ID3, USER2, APP_B).unwrap();
        get_db(&mut sk_state).add_vm_id(&VM_ID4, USER2, CORE_APP_A).unwrap();
        get_db(&mut sk_state).add_vm_id(&VM_ID5, USER3, APP_C).unwrap();

        assert_eq!(vec![VM_ID1, VM_ID2], get_db(&mut sk_state).vm_ids_for_user(USER1).unwrap());
        assert_eq!(
            vec![VM_ID1, VM_ID2],
            get_db(&mut sk_state).vm_ids_for_app(USER1, APP_A).unwrap()
        );
        assert_eq!(vec![VM_ID3], get_db(&mut sk_state).vm_ids_for_app(USER2, APP_B).unwrap());
        assert_eq!(vec![VM_ID5], get_db(&mut sk_state).vm_ids_for_user(USER3).unwrap());

        // Perform a reconciliation and pretend that USER1 and [CORE_APP_A, APP_B] are gone.
        let reconciliation =
            Reconciliation { gone_users: vec![USER1], gone_apps: vec![CORE_APP_A, APP_B] };
        let callback = BnVirtualizationReconciliationCallback::new_binder(
            reconciliation,
            binder::BinderFeatures::default(),
        );
        sk_state.reconcile(&callback).unwrap();

        let empty: Vec<VmId> = Vec::new();
        assert_eq!(empty, get_db(&mut sk_state).vm_ids_for_user(USER1).unwrap());
        assert_eq!(empty, get_db(&mut sk_state).vm_ids_for_app(USER1, APP_A).unwrap());
        // VM for core app stays even though it's reported as absent.
        assert_eq!(vec![VM_ID4], get_db(&mut sk_state).vm_ids_for_user(USER2).unwrap());
        assert_eq!(empty, get_db(&mut sk_state).vm_ids_for_app(USER2, APP_B).unwrap());
        assert_eq!(vec![VM_ID5], get_db(&mut sk_state).vm_ids_for_user(USER3).unwrap());
    }

    #[test]
    fn test_sk_state_too_many_vms() {
        let history = Arc::new(Mutex::new(Vec::new()));
        let mut sk_state = new_test_state(history.clone(), 20);

        // Every VM ID added up to the limit is kept.
        for idx in 0..MAX_VM_IDS_PER_APP {
            let mut vm_id = [0u8; 64];
            vm_id[0..8].copy_from_slice(&(idx as u64).to_be_bytes());
            sk_state.add_id(&vm_id, USER1 as u32, APP_A as u32).unwrap();
            assert_eq!(idx + 1, get_db(&mut sk_state).count_vm_ids_for_app(USER1, APP_A).unwrap());
        }
        assert_eq!(
            MAX_VM_IDS_PER_APP,
            get_db(&mut sk_state).count_vm_ids_for_app(USER1, APP_A).unwrap()
        );

        // Beyond the limit it's one in, one out.
        for idx in MAX_VM_IDS_PER_APP..MAX_VM_IDS_PER_APP + 10 {
            let mut vm_id = [0u8; 64];
            vm_id[0..8].copy_from_slice(&(idx as u64).to_be_bytes());
            sk_state.add_id(&vm_id, USER1 as u32, APP_A as u32).unwrap();
            assert_eq!(
                MAX_VM_IDS_PER_APP,
                get_db(&mut sk_state).count_vm_ids_for_app(USER1, APP_A).unwrap()
            );
        }
        assert_eq!(
            MAX_VM_IDS_PER_APP,
            get_db(&mut sk_state).count_vm_ids_for_app(USER1, APP_A).unwrap()
        );
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

        get_db(&mut sk_state).add_vm_id(&VM_ID1, USER1, APP_A).unwrap();
        get_db(&mut sk_state).add_vm_id(&VM_ID2, USER1, APP_A).unwrap();
        get_db(&mut sk_state).add_vm_id(&VM_ID3, USER2, APP_B).unwrap();
        get_db(&mut sk_state).add_vm_id(&VM_ID5, USER3, APP_C).unwrap();
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
