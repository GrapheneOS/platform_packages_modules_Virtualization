#!/bin/bash

# Only a user account in an interactive shell can run this script.
if [[ "$USER" == "droid" && -n "$PS1" ]]; then
  if grep -q -w "gfxstream_enabled" /proc/cmdline; then
    source /usr/local/bin/enable_gfxstream
  else
    source /usr/local/bin/enable_display
  fi
fi
