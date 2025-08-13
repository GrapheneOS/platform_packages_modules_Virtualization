# Service VM

The Service VM is a lightweight, bare-metal virtual machine specifically
designed to run various services for other virtual machines. It fulfills the
following requirements:

-   Only one instance of the Service VM is allowed to run at any given time.
-   The instance ID of the Service VM remains unchanged during updates of
    both the client VMs and the Service VM.

The instance ID is incorporated into the [CDI values][cdi] calculation of
each VM loaded by pVM Firmware to ensure consistent CDI values for the VM
across all reboots.

[cdi]: https://android.googlesource.com/platform/external/open-dice/+/main/docs/specification.md#CDI-Values

## Architecture

[service_vm][service_vm] is used as the bare-metal kernel for the Service VM. It
shares some low-level setup, such as memory management and virtio device
parsing, with pvmfw. The common setup code is grouped in [libvmbase/][libvmbase].

## Functionality

The main functionality of the Service VM is to process requests from the host
and provide responses for each request. The requests and responses are
serialized in CBOR format and transmitted over a virtio-vsock device.

-   [libservice_vm_comm][libservice_vm_comm] contains the definitions for the
    requests and responses.
-   [libservice_vm_requests][libservice_vm_requests] contains the library that
    processes the requests.
-   [libservice_vm_manager][libservice_vm_manager] manages the Service VM
    session, ensuring that only one Service VM is active at any given time. The
    [virtualizationservice][virtualizationservice] process owns and manages the
    Service VM instance.

[service_vm]: ../guest/service_vm
[libvmbase]: ../libs/libvmbase
[libservice_vm_comm]: ../libs/libservice_vm_comm
[libservice_vm_requests]: ../libs/libservice_vm_requests
[libservice_vm_manager]: ../libs/libservice_vm_manager
[virtualizationservice]: ../android/virtualizationservice

### RKP (Remote Key Provisioning)

The Service VM's primary function is to facilitate VM remote attestation
through [Remote Key Provisioning (RKP)][rkp]. To perform this task, the Service
VM undergoes validation by the RKP Server. It then operates as a remotely
provisioned component that verifies the integrity of other virtual machines.
For details, see [VM remote attestation][vm-attestation].

[rkp]: https://source.android.com/docs/core/ota/modular-system/remote-key-provisioning
[vm-attestation]: https://android.googlesource.com/platform/packages/modules/Virtualization/+/main/docs/vm_remote_attestation.md
