LOCAL_DIR := $(GET_LOCAL_DIR)
MODULE := $(LOCAL_DIR)
MODULE_CRATE_NAME := hypervisor_backends
MODULE_SRCS := \
	$(LOCAL_DIR)/src/lib.rs \

MODULE_LIBRARY_DEPS := \
	trusty/user/base/lib/liballoc-rust \
	$(call FIND_CRATE,once_cell) \
	$(call FIND_CRATE,smccc) \
	$(call FIND_CRATE,uuid) \

include make/library.mk