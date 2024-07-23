// Copyright 2023, The Android Open Source Project
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

//! Supports for the communication between rialto and host.

use crate::error::Result;
use ciborium_io::{Read, Write};
use core::hint::spin_loop;
use core::mem;
use core::result;
use log::info;
use service_vm_comm::{Response, ServiceVmRequest};
use tinyvec::ArrayVec;
use virtio_drivers::{
    self,
    device::socket::{
        SocketError, VirtIOSocket, VsockAddr, VsockConnectionManager, VsockEventType,
    },
    transport::Transport,
    Hal,
};

const WRITE_BUF_CAPACITY: usize = 512;

pub struct VsockStream<H: Hal, T: Transport> {
    connection_manager: VsockConnectionManager<H, T>,
    /// Peer address. The same port is used on rialto and peer for convenience.
    peer_addr: VsockAddr,
    write_buf: ArrayVec<[u8; WRITE_BUF_CAPACITY]>,
}

impl<H: Hal, T: Transport> VsockStream<H, T> {
    pub fn new(
        socket_device_driver: VirtIOSocket<H, T>,
        peer_addr: VsockAddr,
    ) -> virtio_drivers::Result<Self> {
        let mut vsock_stream = Self {
            connection_manager: VsockConnectionManager::new(socket_device_driver),
            peer_addr,
            write_buf: ArrayVec::default(),
        };
        vsock_stream.connect()?;
        Ok(vsock_stream)
    }

    fn connect(&mut self) -> virtio_drivers::Result {
        self.connection_manager.connect(self.peer_addr, self.peer_addr.port)?;
        self.wait_for_connect()?;
        info!("Connected to the peer {:?}", self.peer_addr);
        Ok(())
    }

    fn wait_for_connect(&mut self) -> virtio_drivers::Result {
        loop {
            if let Some(event) = self.poll_event_from_peer()? {
                match event {
                    VsockEventType::Connected => return Ok(()),
                    VsockEventType::Disconnected { .. } => {
                        return Err(SocketError::ConnectionFailed.into())
                    }
                    // We shouldn't receive the following event before the connection is
                    // established.
                    VsockEventType::ConnectionRequest | VsockEventType::Received { .. } => {
                        return Err(SocketError::InvalidOperation.into())
                    }
                    // We can receive credit requests and updates at any time.
                    // This can be ignored as the connection manager handles them in poll().
                    VsockEventType::CreditRequest | VsockEventType::CreditUpdate => {}
                }
            } else {
                spin_loop();
            }
        }
    }

    pub fn read_request(&mut self) -> Result<ServiceVmRequest> {
        Ok(ciborium::from_reader(self)?)
    }

    pub fn write_response(&mut self, response: &Response) -> Result<()> {
        Ok(ciborium::into_writer(response, self)?)
    }

    /// Shuts down the data channel.
    pub fn shutdown(&mut self) -> virtio_drivers::Result {
        self.connection_manager.force_close(self.peer_addr, self.peer_addr.port)?;
        info!("Connection shutdown.");
        Ok(())
    }

    fn recv(&mut self, buffer: &mut [u8]) -> virtio_drivers::Result<usize> {
        let bytes_read =
            self.connection_manager.recv(self.peer_addr, self.peer_addr.port, buffer)?;

        let buffer_available_bytes = self
            .connection_manager
            .recv_buffer_available_bytes(self.peer_addr, self.peer_addr.port)?;
        if buffer_available_bytes == 0 && bytes_read > 0 {
            self.connection_manager.update_credit(self.peer_addr, self.peer_addr.port)?;
        }
        Ok(bytes_read)
    }

    fn wait_for_send(&mut self, buffer: &[u8]) -> virtio_drivers::Result {
        const INSUFFICIENT_BUFFER_SPACE_ERROR: virtio_drivers::Error =
            virtio_drivers::Error::SocketDeviceError(SocketError::InsufficientBufferSpaceInPeer);
        loop {
            match self.connection_manager.send(self.peer_addr, self.peer_addr.port, buffer) {
                Ok(_) => return Ok(()),
                Err(INSUFFICIENT_BUFFER_SPACE_ERROR) => {
                    self.poll()?;
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn wait_for_recv(&mut self) -> virtio_drivers::Result {
        loop {
            match self.poll()? {
                Some(VsockEventType::Received { .. }) => return Ok(()),
                _ => spin_loop(),
            }
        }
    }

    /// Polls the rx queue after the connection is established with the peer, this function
    /// rejects some invalid events. The valid events are handled inside the connection
    /// manager.
    fn poll(&mut self) -> virtio_drivers::Result<Option<VsockEventType>> {
        if let Some(event) = self.poll_event_from_peer()? {
            match event {
                VsockEventType::Disconnected { .. } => Err(SocketError::ConnectionFailed.into()),
                VsockEventType::Connected | VsockEventType::ConnectionRequest => {
                    Err(SocketError::InvalidOperation.into())
                }
                // When there is a received event, the received data is buffered in the
                // connection manager's internal receive buffer, so we don't need to do
                // anything here.
                // The credit request and updates also handled inside the connection
                // manager.
                VsockEventType::Received { .. }
                | VsockEventType::CreditRequest
                | VsockEventType::CreditUpdate => Ok(Some(event)),
            }
        } else {
            Ok(None)
        }
    }

    fn poll_event_from_peer(&mut self) -> virtio_drivers::Result<Option<VsockEventType>> {
        Ok(self.connection_manager.poll()?.map(|event| {
            assert_eq!(event.source, self.peer_addr);
            assert_eq!(event.destination.port, self.peer_addr.port);
            event.event_type
        }))
    }
}

impl<H: Hal, T: Transport> Read for VsockStream<H, T> {
    type Error = virtio_drivers::Error;

    fn read_exact(&mut self, data: &mut [u8]) -> result::Result<(), Self::Error> {
        let mut start = 0;
        while start < data.len() {
            let len = self.recv(&mut data[start..])?;
            let len = if len == 0 {
                self.wait_for_recv()?;
                self.recv(&mut data[start..])?
            } else {
                len
            };
            start += len;
        }
        Ok(())
    }
}

impl<H: Hal, T: Transport> Write for VsockStream<H, T> {
    type Error = virtio_drivers::Error;

    fn write_all(&mut self, data: &[u8]) -> result::Result<(), Self::Error> {
        if data.len() >= self.write_buf.capacity() - self.write_buf.len() {
            self.flush()?;
            if data.len() >= self.write_buf.capacity() {
                self.wait_for_send(data)?;
                return Ok(());
            }
        }
        self.write_buf.extend_from_slice(data);
        Ok(())
    }

    fn flush(&mut self) -> result::Result<(), Self::Error> {
        if !self.write_buf.is_empty() {
            // We need to take the memory from self.write_buf to a temporary
            // buffer to avoid borrowing `*self` as mutable and immutable on
            // the same time in `self.wait_for_send(&self.write_buf)`.
            let buffer = mem::take(&mut self.write_buf);
            self.wait_for_send(&buffer)?;
        }
        Ok(())
    }
}
