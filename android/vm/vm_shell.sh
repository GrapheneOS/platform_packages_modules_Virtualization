#!/bin/bash

# Copyright 2020 Google Inc. All rights reserved.
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

# vm_shell.sh: utilities to interact with Microdroid VMs

function print_help() {
    echo "vm_shell.sh provides utilities to interact with Microdroid VMs"
    echo ""
    echo "Available commands:"
    echo "    connect [cid] - establishes adb connection with the VM"
    echo "      cid - cid of the VM to connect to. If not specified user will "
    echo "            be promted to select one from the list of available cids"
    echo ""
    echo "    start-microdroid [--auto-connect] [-- extra_args]"
    echo "        Starts a Microdroid VM. Args after the -- will be"
    echo "        passed through to the invocation of the "
    echo "        /apex/com.android.virt/bin/vm run-microdroid binary."
    echo ""
    echo "        E.g.:"
    echo "            vm_shell start-microdroid -- --cpu 5"
    echo ""
    echo "        --auto-connect - automatically connects to the started VMs"
    echo ""
    echo "    help - prints this help message"
}

function connect_vm() {
    cid=$1
    echo Connecting to CID ${cid}
    adb disconnect localhost:8000 2>/dev/null
    adb forward tcp:8000 vsock:${cid}:5555
    adb connect localhost:8000
    adb -s localhost:8000 root
    adb -s localhost:8000 wait-for-device
    adb -s localhost:8000 shell
    exit 0
}

function list_cids() {
    local selected_cid=$1
    local available_cids=$(adb shell /apex/com.android.virt/bin/vm list | awk 'BEGIN { FS="[:,]" } /cid/ { print $2; }')
    echo "${available_cids}"
}

function handle_connect_cmd() {
    selected_cid=$1

    available_cids=$(list_cids)

    if [ -z "${available_cids}" ]; then
        echo No VM is available
        exit 1
    fi

    if [ ! -n "${selected_cid}" ]; then
        if [ ${#selected_cid[@]} -eq 1 ]; then
            selected_cid=${available_cids[0]}
        else
            PS3="Select CID of VM to adb-shell into: "
            select cid in ${available_cids}
            do
                selected_cid=${cid}
                break
            done
        fi
    fi

    if [[ ! " ${available_cids[*]} " =~ " ${selected_cid} " ]]; then
        echo VM of CID $selected_cid does not exist. Available CIDs: ${available_cids}
        exit 1
    fi

    connect_vm ${selected_cid}
}

function handle_start_microdroid_cmd() {
    while [[ "$#" -gt 0 ]]; do
        case $1 in
          --auto-connect) auto_connect=true; ;;
          --) shift; passthrough_args="$@"; break ;;
          *) echo "Unknown argument: $1"; exit 1 ;;
        esac
        shift
    done
    if [[ "${auto_connect}" == true ]]; then
        adb shell /apex/com.android.virt/bin/vm run-microdroid "${passthrough_args}" &
        trap "kill $!" EXIT
        sleep 2
        handle_connect_cmd
    else
        adb shell /apex/com.android.virt/bin/vm run-microdroid "${passthrough_args}"
    fi
}

cmd=$1
shift

case $cmd in
  connect) handle_connect_cmd "$@" ;;
  start-microdroid) handle_start_microdroid_cmd "$@" ;;
  help) print_help ;;
  *) print_help; exit 1 ;;
esac
