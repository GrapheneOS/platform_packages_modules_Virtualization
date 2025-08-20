#!/bin/bash

# Only a user account can run this script.
if [[ "$USER" == "droid" ]]; then
  if grep -q -w "gfxstream_enabled" /proc/cmdline; then
    source /usr/local/bin/enable_gfxstream
  else
    source /usr/local/bin/enable_display
  fi
  echo "Display is enabled. Please open a display activity before running any GUI applications."
fi
