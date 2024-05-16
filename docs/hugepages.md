# Huge Pages

From Android 15, the pKVM hypervisor supports Transparent Hugepages. This is a
Linux feature which allows the kernel to allocate, when possible, a huge-page
(typically, 2MiB on a 4K system). This huge-page being the size of a block,
the hypervisor can leverage this allocation to also use a block mapping
in the stage-2 page tables, instead of 512 individual contiguous single page
mappings.

Using block mappings brings a significant performance improvement by reducing
the number of stage-2 page faults as well as the TLB pressure. However, finding
a huge-page can be difficult on a system where the memory is fragmented.

By default, huge-pages are disabled.

## Enabling THP

### 1. Sysfs configuration

The sysfs configuration file that will enable THP for AVF is

```
/sys/kernel/mm/transparent_hugepages/shmem_enabled
```

This always defaults to `never`. It is recommended to set it to `advise` to
benefit from the THP performance improvement.

THPs can have an impact on the system depending on the chosen policy. The
policy is configured with the following sysfs file:

```
/sys/kernel/mm/transparent_hugepages/defrag
```

The recommended policy is `never` as this has zero impact on the system. THPs
would be used only if some are available.

More information can be found in the Linux
[admin guide](https://docs.kernel.org/admin-guide/mm/transhuge.html).

### 2. AVF configuration

The guest VM configuration can select huge-pages with the `vm_config.json`
option `"hugepages": true`.

Alternatively, the `vm` command can also pass `--hugepages`.
