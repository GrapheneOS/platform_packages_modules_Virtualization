#!/bin/bash

# Auto-run restore-backup if backup is mounted and not yet restored.
if [[ "$USER" == "droid" && -n "$PS1" ]]; then
  if [ ! -f /home/droid/.restore_complete ] && mountpoint -q /mnt/backup; then
    sudo /usr/local/bin/restore-backup
  fi
fi
