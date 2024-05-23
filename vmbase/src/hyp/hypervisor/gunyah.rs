use super::common::Hypervisor;
use uuid::{uuid, Uuid};

pub(super) struct GunyahHypervisor;

impl GunyahHypervisor {
    pub const UUID: Uuid = uuid!("c1d58fcd-a453-5fdb-9265-ce36673d5f14");
}

impl Hypervisor for GunyahHypervisor {}
