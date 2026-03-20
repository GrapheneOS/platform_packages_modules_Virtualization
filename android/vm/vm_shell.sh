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
    echo "    connect [cid|name] - establishes adb connection with the VM"
    echo "      cid|name - either CID or name of the VM to connect to. If not "
    echo "            specified, user will be prompted to select one from the "
    echo "            list of available VMs."
    echo ""
    echo "    start-microdroid [--auto-connect] [-- extra_args]"
    echo "        Starts a Microdroid VM. Args after the -- will be"
    echo "        passed through to the invocation of the "
    echo "        /apex/com.android.virt/bin/vm run-microdroid binary."
    echo ""
    echo "        E.g.:"
    echo "            vm_shell start-microdroid -- --protected --debug full"
    echo ""
    echo "        --auto-connect - automatically connects to the started VMs"
    echo ""
    echo "    help - prints this help message"
}

function connect_vm() {
    cid=$1
    echo "Starting adbd in a VM with CID ${cid}"
    adb shell /apex/com.android.virt/bin/vm start-adbd --cid=${cid}
    echo Connecting to CID "${cid}"
    adb disconnect localhost:8000 2>/dev/null
    adb forward tcp:8000 "vsock:${cid}:5555"
    adb connect localhost:8000
    adb -s localhost:8000 root
    adb -s localhost:8000 wait-for-device
    adb -s localhost:8000 shell
    exit 0
}

function list_vms() {
    if adb devices | grep -q "^localhost:8000"; then
      echo "WARNING: localhost:8000 is already listed in adb devices.">&2
      echo "There could be an open terminal connected to the adb console.">&2
      read -r -p "Do you want to continue? (y/N): " choice
      if [[ "$choice" != "y" ]]; then
        echo "Exiting.">&2
        exit 1
      else
        adb disconnect localhost:8000 >/dev/null 2>&1
      fi
    fi
    declare -n vms="$1"
    while IFS= read -r line; do
      eval "$line"
      if [[ -n "$cid" && -n "$name" ]]; then
        vms["$cid"]="$name"
        unset cid name
      fi
    done < <(adb shell vm list | awk '
      /name:/ {
          name = $0; sub(/.*name: "/, "", name); sub(/".*/, "", name);
      }
      /cid:/ {
          cid = $0; sub(/.*cid: /, "", cid); sub(/,.*/, "", cid);
          printf "cid=%s name=%s\n", cid, name;
          name = "";
      }
   ')
}

function select_vm() {
    declare -n vms="$1"
    if [ ${#vms[@]} -eq 1 ]; then
        selected_cid=${!vms[@]}
    else
        PS3="Select VM to adb-shell into: "
        menu_items=()
        for cid in "${!vms[@]}"; do
            menu_items+=("${vms["$cid"]} (cid: $cid)")
        done
        select selection in "${menu_items[@]}" "Quit"; do
            if [ "$selection" == "Quit" ]; then
                exit 1
            elif [[ "$selection" =~ ^(.*)\ \(cid:\ ([0-9]+)\)$ ]]; then
                selected_cid="${BASH_REMATCH[2]}"
                break
            fi
        done
    fi
    echo "$selected_cid"
}

function handle_connect_cmd() {
    local cid_or_name=$1
    declare -A vm_list

    list_vms "vm_list"

    if [ "${#vm_list[@]}" -eq 0 ]; then
        echo "No VM is available."
    fi

    if [ -z "${cid_or_name}" ]; then
        # If neither cid or name is given, let user select from the available VMs list
        selected_cid=$(select_vm "vm_list")
    else
        # If cid or name is given, match against cid first, and then fall back to match
        # name. When multiple VMs match the same name, let user select.
        if [ -v vm_list["$cid_or_name"] ]; then
            selected_cid="$cid_or_name"
        else
            declare -A matched_vm_list
            for cid in "${!vm_list[@]}"; do
                if [ "${vm_list["$cid"]}" == "$cid_or_name" ]; then
                    matched_vm_list["$cid"]="$cid_or_name"
                fi
            done
            if [ "${#matched_vm_list[@]}" -eq 0 ]; then
                echo "No VM matches "${cid_or_name}"."
            else
                selected_cid=$(select_vm "matched_vm_list")
            fi
        fi
    fi

    if [ -z "${selected_cid}" ]; then
        exit 1
    fi
    connect_vm "${selected_cid}"
}

function handle_start_microdroid_cmd() {
    while [[ "$#" -gt 0 ]]; do
        case $1 in
          --auto-connect) auto_connect=true; ;;
          --) shift; passthrough_args=("$@"); break ;;
          *) echo "Unknown argument: $1"; exit 1 ;;
        esac
        shift
    done
    if [[ "${auto_connect}" == true ]]; then
        temp_file=$(mktemp)
        ( adb shell /apex/com.android.virt/bin/vm run-microdroid "${passthrough_args[@]}" |& tee "${temp_file}" ) &
        last_pid="$!"

        trap "pkill -P ${last_pid} && rm ${temp_file} && adb disconnect localhost:8000" EXIT

        # Wait for the VM to be fully booted
        while true; do
          sleep 1
          grep "${temp_file}" -e 'payload is ready' && break
        done

        cid=$(sed -n 's/^Created.*with\ CID\ \([0-9]*\),.*$/\1/p' "${temp_file}")

        if [[ -z "${cid}" ]]; then
          echo "Fail to find CID of launched VM" >&2
          exit 1
        fi
        connect_vm "${cid}"
    else
        adb shell /apex/com.android.virt/bin/vm run-microdroid "${passthrough_args[@]}"
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
