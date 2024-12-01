// Copyright 2024 The Android Open Source Project
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

// Copied from ChromiumOS with relicensing:
// src/platform2/vm_tools/chunnel/src/bin/chunneld.rs

//! Host-side stream socket forwarder

use std::collections::btree_map::Entry as BTreeMapEntry;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fmt;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, TcpListener};
use std::os::unix::io::AsRawFd;
use std::result;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use forwarder::forwarder::ForwarderSession;
use jni::objects::{JIntArray, JObject, JValue};
use jni::sys::jint;
use jni::JNIEnv;
use log::{debug, error, info, warn};
use nix::sys::eventfd::EventFd;
use poll_token_derive::PollToken;
use vmm_sys_util::poll::{PollContext, PollToken};
use vsock::VsockListener;
use vsock::VMADDR_CID_ANY;

const CHUNNEL_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

const VMADDR_PORT_ANY: u32 = u32::MAX;

static SHUTDOWN_EVT: LazyLock<EventFd> =
    LazyLock::new(|| EventFd::new().expect("Could not create shutdown eventfd"));

static UPDATE_EVT: LazyLock<EventFd> =
    LazyLock::new(|| EventFd::new().expect("Could not create update eventfd"));

static UPDATE_QUEUE: LazyLock<Arc<Mutex<VecDeque<u16>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(VecDeque::new())));

#[remain::sorted]
#[derive(Debug)]
enum Error {
    BindVsock(io::Error),
    IncorrectCid(u32),
    LaunchForwarderGuest(jni::errors::Error),
    NoListenerForPort(u16),
    NoSessionForTag(SessionTag),
    PollContextAdd(vmm_sys_util::errno::Error),
    PollContextDelete(vmm_sys_util::errno::Error),
    PollContextNew(vmm_sys_util::errno::Error),
    PollWait(vmm_sys_util::errno::Error),
    SetVsockNonblocking(io::Error),
    TcpAccept(io::Error),
    TcpListenerPort(io::Error),
    UpdateEventRead(nix::Error),
    VsockAccept(io::Error),
    VsockAcceptTimeout,
    VsockListenerPort(io::Error),
}

type Result<T> = result::Result<T, Error>;

impl fmt::Display for Error {
    #[remain::check]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use self::Error::*;

        #[remain::sorted]
        match self {
            BindVsock(e) => write!(f, "failed to bind vsock: {}", e),
            IncorrectCid(cid) => write!(f, "chunnel connection from unexpected cid {}", cid),
            LaunchForwarderGuest(e) => write!(f, "failed to launch forwarder_guest {}", e),
            NoListenerForPort(port) => write!(f, "could not find listener for port: {}", port),
            NoSessionForTag(tag) => write!(f, "could not find session for tag: {:x}", tag),
            PollContextAdd(e) => write!(f, "failed to add fd to poll context: {}", e),
            PollContextDelete(e) => write!(f, "failed to delete fd from poll context: {}", e),
            PollContextNew(e) => write!(f, "failed to create poll context: {}", e),
            PollWait(e) => write!(f, "failed to wait for poll: {}", e),
            SetVsockNonblocking(e) => write!(f, "failed to set vsock to nonblocking: {}", e),
            TcpAccept(e) => write!(f, "failed to accept tcp: {}", e),
            TcpListenerPort(e) => {
                write!(f, "failed to read local sockaddr for tcp listener: {}", e)
            }
            UpdateEventRead(e) => write!(f, "failed to read update eventfd: {}", e),
            VsockAccept(e) => write!(f, "failed to accept vsock: {}", e),
            VsockAcceptTimeout => write!(f, "timed out waiting for vsock connection"),
            VsockListenerPort(e) => write!(f, "failed to get vsock listener port: {}", e),
        }
    }
}

/// A tag that uniquely identifies a particular forwarding session. This has arbitrarily been
/// chosen as the fd of the local (TCP) socket.
type SessionTag = u32;

/// Implements PollToken for chunneld's main poll loop.
#[derive(Clone, Copy, PollToken)]
enum Token {
    Shutdown,
    UpdatePorts,
    Ipv4Listener(u16),
    Ipv6Listener(u16),
    LocalSocket(SessionTag),
    RemoteSocket(SessionTag),
}

/// PortListeners includes all listeners (IPv4 and IPv6) for a given port, and the target
/// container.
struct PortListeners {
    tcp4_listener: TcpListener,
    tcp6_listener: TcpListener,
}

/// SocketFamily specifies whether a socket uses IPv4 or IPv6.
enum SocketFamily {
    Ipv4,
    Ipv6,
}

/// ForwarderSessions encapsulates all forwarding state for chunneld.
struct ForwarderSessions<'a> {
    listening_ports: BTreeMap<u16, PortListeners>,
    tcp4_forwarders: HashMap<SessionTag, ForwarderSession>,
    cid: u32,
    jni_env: JNIEnv<'a>,
    jni_cb: JObject<'a>,
}

impl<'a> ForwarderSessions<'a> {
    /// Creates a new instance of ForwarderSessions.
    fn new(cid: i32, jni_env: JNIEnv<'a>, jni_cb: JObject<'a>) -> Result<Self> {
        Ok(ForwarderSessions {
            listening_ports: BTreeMap::new(),
            tcp4_forwarders: HashMap::new(),
            cid: cid as u32,
            jni_env,
            jni_cb,
        })
    }

    /// Adds or removes listeners based on the latest listening ports from the D-Bus thread.
    fn process_update_queue(&mut self, poll_ctx: &PollContext<Token>) -> Result<()> {
        // Unwrap of LockResult is customary.
        let mut update_queue = UPDATE_QUEUE.lock().unwrap();
        let mut active_ports: BTreeSet<u16> = BTreeSet::new();

        // Add any new listeners first.
        while let Some(port) = update_queue.pop_front() {
            // Ignore privileged ports.
            if port < 1024 {
                continue;
            }
            if let BTreeMapEntry::Vacant(o) = self.listening_ports.entry(port) {
                // Failing to bind a port is not fatal, but we should log it.
                // Both IPv4 and IPv6 localhost must be bound since the host may resolve
                // "localhost" to either.
                let tcp4_listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
                    Ok(listener) => listener,
                    Err(e) => {
                        warn!("failed to bind TCPv4 port: {}", e);
                        continue;
                    }
                };
                let tcp6_listener = match TcpListener::bind((Ipv6Addr::LOCALHOST, port)) {
                    Ok(listener) => listener,
                    Err(e) => {
                        warn!("failed to bind TCPv6 port: {}", e);
                        continue;
                    }
                };
                poll_ctx
                    .add(&tcp4_listener, Token::Ipv4Listener(port))
                    .map_err(Error::PollContextAdd)?;
                poll_ctx
                    .add(&tcp6_listener, Token::Ipv6Listener(port))
                    .map_err(Error::PollContextAdd)?;
                o.insert(PortListeners { tcp4_listener, tcp6_listener });
            }
            active_ports.insert(port);
        }

        // Iterate over the existing listeners; if the port is no longer in the
        // listener list, remove it.
        let old_ports: Vec<u16> = self.listening_ports.keys().cloned().collect();
        for port in old_ports.iter() {
            if !active_ports.contains(port) {
                // Remove the PortListeners struct first - on error we want to drop it and the
                // fds it contains.
                let _listening_port = self.listening_ports.remove(port);
            }
        }

        // Consume the eventfd.
        UPDATE_EVT.read().map_err(Error::UpdateEventRead)?;

        Ok(())
    }

    fn accept_connection(
        &mut self,
        poll_ctx: &PollContext<Token>,
        port: u16,
        sock_family: SocketFamily,
    ) -> Result<()> {
        let port_listeners =
            self.listening_ports.get(&port).ok_or(Error::NoListenerForPort(port))?;

        let listener = match sock_family {
            SocketFamily::Ipv4 => &port_listeners.tcp4_listener,
            SocketFamily::Ipv6 => &port_listeners.tcp6_listener,
        };

        // This session should be dropped if any of the PollContext setup fails. Since the only
        // extant fds for the underlying sockets will be closed, they will be unregistered from
        // epoll set automatically.
        let session =
            create_forwarder_session(listener, self.cid, &mut self.jni_env, &self.jni_cb)?;

        let tag = session.local_stream().as_raw_fd() as u32;

        poll_ctx
            .add(session.local_stream(), Token::LocalSocket(tag))
            .map_err(Error::PollContextAdd)?;
        poll_ctx
            .add(session.remote_stream(), Token::RemoteSocket(tag))
            .map_err(Error::PollContextAdd)?;

        self.tcp4_forwarders.insert(tag, session);

        Ok(())
    }

    fn forward_from_local(&mut self, poll_ctx: &PollContext<Token>, tag: SessionTag) -> Result<()> {
        let session = self.tcp4_forwarders.get_mut(&tag).ok_or(Error::NoSessionForTag(tag))?;
        let shutdown = session.forward_from_local().unwrap_or(true);
        if shutdown {
            poll_ctx.delete(session.local_stream()).map_err(Error::PollContextDelete)?;
            if session.is_shut_down() {
                self.tcp4_forwarders.remove(&tag);
            }
        }

        Ok(())
    }

    fn forward_from_remote(
        &mut self,
        poll_ctx: &PollContext<Token>,
        tag: SessionTag,
    ) -> Result<()> {
        let session = self.tcp4_forwarders.get_mut(&tag).ok_or(Error::NoSessionForTag(tag))?;
        let shutdown = session.forward_from_remote().unwrap_or(true);
        if shutdown {
            poll_ctx.delete(session.remote_stream()).map_err(Error::PollContextDelete)?;
            if session.is_shut_down() {
                self.tcp4_forwarders.remove(&tag);
            }
        }

        Ok(())
    }

    fn run(&mut self) -> Result<()> {
        let poll_ctx: PollContext<Token> = PollContext::new().map_err(Error::PollContextNew)?;
        poll_ctx.add(&*UPDATE_EVT, Token::UpdatePorts).map_err(Error::PollContextAdd)?;
        poll_ctx.add(&*SHUTDOWN_EVT, Token::Shutdown).map_err(Error::PollContextAdd)?;

        loop {
            let events = poll_ctx.wait().map_err(Error::PollWait)?;

            for event in events.iter_readable() {
                match event.token() {
                    Token::Shutdown => {
                        return Ok(());
                    }
                    Token::UpdatePorts => {
                        if let Err(e) = self.process_update_queue(&poll_ctx) {
                            error!("error updating listening ports: {}", e);
                        }
                    }
                    Token::Ipv4Listener(port) => {
                        if let Err(e) = self.accept_connection(&poll_ctx, port, SocketFamily::Ipv4)
                        {
                            error!("error accepting connection: {}", e);
                        }
                    }
                    Token::Ipv6Listener(port) => {
                        if let Err(e) = self.accept_connection(&poll_ctx, port, SocketFamily::Ipv6)
                        {
                            error!("error accepting connection: {}", e);
                        }
                    }
                    Token::LocalSocket(tag) => {
                        if let Err(e) = self.forward_from_local(&poll_ctx, tag) {
                            error!("error forwarding local traffic: {}", e);
                        }
                    }
                    Token::RemoteSocket(tag) => {
                        if let Err(e) = self.forward_from_remote(&poll_ctx, tag) {
                            error!("error forwarding remote traffic: {}", e);
                        }
                    }
                }
            }
        }
    }
}

/// Creates a forwarder session from a `listener` that has a pending connection to accept.
fn create_forwarder_session(
    listener: &TcpListener,
    cid: u32,
    jni_env: &mut JNIEnv,
    jni_cb: &JObject,
) -> Result<ForwarderSession> {
    let (tcp_stream, _) = listener.accept().map_err(Error::TcpAccept)?;
    // Bind a vsock port, tell the guest to connect, and accept the connection.
    let vsock_listener = VsockListener::bind_with_cid_port(VMADDR_CID_ANY, VMADDR_PORT_ANY)
        .map_err(Error::BindVsock)?;
    vsock_listener.set_nonblocking(true).map_err(Error::SetVsockNonblocking)?;

    let tcp4_port = listener.local_addr().map_err(Error::TcpListenerPort)?.port();
    let vsock_port = vsock_listener.local_addr().map_err(Error::VsockListenerPort)?.port();
    jni_env
        .call_method(
            jni_cb,
            "onForwardingRequestReceived",
            "(II)V",
            &[JValue::Int(tcp4_port.into()), JValue::Int(vsock_port as i32)],
        )
        .map_err(Error::LaunchForwarderGuest)?;

    #[derive(PollToken)]
    enum Token {
        VsockAccept,
    }

    let poll_ctx: PollContext<Token> = PollContext::new().map_err(Error::PollContextNew)?;
    poll_ctx.add(&vsock_listener, Token::VsockAccept).map_err(Error::PollContextAdd)?;

    // Wait a few seconds for the guest to connect.
    let events = poll_ctx.wait_timeout(CHUNNEL_CONNECT_TIMEOUT).map_err(Error::PollWait)?;

    match events.iter_readable().next() {
        Some(_) => {
            let (vsock_stream, sockaddr) = vsock_listener.accept().map_err(Error::VsockAccept)?;

            if sockaddr.cid() != cid {
                Err(Error::IncorrectCid(sockaddr.cid()))
            } else {
                Ok(ForwarderSession::new(tcp_stream.into(), vsock_stream.into()))
            }
        }
        None => Err(Error::VsockAcceptTimeout),
    }
}

// TODO(b/340126051): Host can receive opened ports from the guest.
fn run_forwarder_host(cid: i32, jni_env: JNIEnv, jni_cb: JObject) -> Result<()> {
    debug!("Starting forwarder_host");
    let mut sessions = ForwarderSessions::new(cid, jni_env, jni_cb)?;
    sessions.run()
}

/// JNI function for running forwarder_host.
#[no_mangle]
pub extern "C" fn Java_com_android_virtualization_terminal_DebianServiceImpl_runForwarderHost(
    env: JNIEnv,
    _class: JObject,
    cid: jint,
    callback: JObject,
) {
    // Clear shutdown event FD before running forwarder host.
    SHUTDOWN_EVT.write(1).expect("Failed to write shutdown event FD");
    SHUTDOWN_EVT.read().expect("Failed to consume shutdown event FD");

    match run_forwarder_host(cid, env, callback) {
        Ok(_) => {
            info!("forwarder_host is terminated");
        }
        Err(e) => {
            error!("Error on forwarder_host: {:?}", e);
        }
    }
}

/// JNI function for terminating forwarder_host.
#[no_mangle]
pub extern "C" fn Java_com_android_virtualization_terminal_DebianServiceImpl_terminateForwarderHost(
    _env: JNIEnv,
    _class: JObject,
) {
    SHUTDOWN_EVT.write(1).expect("Failed to write shutdown event FD");
}

/// JNI function for updating listening ports.
#[no_mangle]
pub extern "C" fn Java_com_android_virtualization_terminal_DebianServiceImpl_updateListeningPorts(
    env: JNIEnv,
    _class: JObject,
    ports: JIntArray,
) {
    let length = env.get_array_length(&ports).expect("Failed to get length of port array");
    let mut buf = vec![0; length as usize];
    env.get_int_array_region(ports, 0, &mut buf).expect("Failed to get port array");

    let mut update_queue = UPDATE_QUEUE.lock().unwrap();
    update_queue.clear();
    for port in buf {
        update_queue.push_back(port.try_into().expect("Failed to add port into update queue"));
    }
    UPDATE_EVT.write(1).expect("failed to write update eventfd");
}
