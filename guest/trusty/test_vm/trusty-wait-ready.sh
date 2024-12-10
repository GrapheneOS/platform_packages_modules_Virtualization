#!/bin/bash
set -euo pipefail

function get_cid {
    local max_cid
    max_cid=$(/apex/com.android.virt/bin/vm list | awk 'BEGIN { FS="[:,]" } /cid/ { print $2; }' | sort -n | tail -1)

    # return the value trimmed from whitespaces
    echo "${max_cid}" | xargs
}

function wait_for_cid {
    TIMES=${1:-20}
    X=0
    local init_cid
    init_cid=$(get_cid)
    while [ "$TIMES" -eq 0 ] || [ "$TIMES" -gt "$X" ]
    do
      local cid
      cid=$(get_cid)
      echo "wait_for_cid: retry $(( X++ )) / $TIMES : init_cid=$init_cid cid=$cid";
      if [ "$cid" -gt "$init_cid" ]
      then
        break
      else
        sleep 2
      fi
    done
    setprop trusty.test_vm.vm_cid "$cid"
}

# This script is expected to be started before the trusty_test_vm is started
# wait_for_cid gets the max cid and wait for it to be updated as an indication
# that the trusty_test_vm has properly started.
# wait_for_cid polls for the CID change at 2 seconds intervals
# the input argument is the max number of retries (20 by default)
wait_for_cid "$@"

echo trusty.test_vm.vm_cid="$(getprop trusty.test_vm.vm_cid)"
