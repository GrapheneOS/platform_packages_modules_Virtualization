// Copyright 2025, The Android Open Source Project
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

//! Proxy to Secretkeeper

use crate::aidl::{self, Arc as AuthgraphArc};
use binder::{self, BinderFeatures, ExceptionCode, Interface, Strong};

pub(crate) const IDENTIFIER: &str = "android.hardware.security.secretkeeper.ISecretkeeper/default";

pub(crate) fn is_supported() -> bool {
    binder::is_declared(IDENTIFIER).expect("Could not check for declared Secretkeeper interface")
}

pub(crate) struct SecretkeeperProxy(pub Strong<dyn aidl::ISecretkeeper>);

impl Interface for SecretkeeperProxy {}

impl aidl::ISecretkeeper for SecretkeeperProxy {
    fn processSecretManagementRequest(&self, req: &[u8]) -> binder::Result<Vec<u8>> {
        // Pass the request to the channel, and read the response.
        self.0.processSecretManagementRequest(req)
    }

    fn getAuthGraphKe(&self) -> binder::Result<Strong<dyn aidl::IAuthGraphKeyExchange>> {
        let ag = AuthGraphKeyExchangeProxy(self.0.getAuthGraphKe()?);
        Ok(aidl::BnAuthGraphKeyExchange::new_binder(ag, BinderFeatures::default()))
    }

    // SecretkeeperProxy is really a RPC binder service for PVM (It is called by
    // MicrodroidManager). PVMs should not be allowed to trigger secrets' deletion.
    fn deleteIds(&self, _ids: &[aidl::SecretId]) -> binder::Result<()> {
        Err(ExceptionCode::UNSUPPORTED_OPERATION.into())
    }

    fn deleteAll(&self) -> binder::Result<()> {
        Err(ExceptionCode::UNSUPPORTED_OPERATION.into())
    }

    fn getSecretkeeperIdentity(&self) -> binder::Result<aidl::PublicKey> {
        // SecretkeeperProxy is really a RPC binder service for PVM (It is called by
        // MicrodroidManager). PVMs do not & must not (for security reason) rely on
        // getSecretKeeperIdentity, so we throw an exception if someone attempts to
        // use this API from the proxy.
        Err(ExceptionCode::SECURITY.into())
    }
}

struct AuthGraphKeyExchangeProxy(Strong<dyn aidl::IAuthGraphKeyExchange>);

impl Interface for AuthGraphKeyExchangeProxy {}

impl aidl::IAuthGraphKeyExchange for AuthGraphKeyExchangeProxy {
    fn create(&self) -> binder::Result<aidl::SessionInitiationInfo> {
        self.0.create()
    }

    fn init(
        &self,
        peer_pub_key: &aidl::PubKey,
        peer_id: &aidl::Identity,
        peer_nonce: &[u8],
        peer_version: i32,
    ) -> binder::Result<aidl::KeInitResult> {
        self.0.init(peer_pub_key, peer_id, peer_nonce, peer_version)
    }

    fn finish(
        &self,
        peer_pub_key: &aidl::PubKey,
        peer_id: &aidl::Identity,
        peer_signature: &aidl::SessionIdSignature,
        peer_nonce: &[u8],
        peer_version: i32,
        own_key: &aidl::Key,
    ) -> binder::Result<aidl::SessionInfo> {
        self.0.finish(peer_pub_key, peer_id, peer_signature, peer_nonce, peer_version, own_key)
    }

    fn authenticationComplete(
        &self,
        peer_signature: &aidl::SessionIdSignature,
        shared_keys: &[AuthgraphArc; 2],
    ) -> binder::Result<[AuthgraphArc; 2]> {
        self.0.authenticationComplete(peer_signature, shared_keys)
    }
}
