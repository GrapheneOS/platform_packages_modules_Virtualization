#!/bin/bash

# Only a user account can run this script.
if [[ "$USER" == "droid" ]]; then
  if [[ -f /mnt/internal/use_gfxstream ]]; then
    source /usr/local/bin/enable_gfxstream
  else
    source /usr/local/bin/enable_display
  fi
  echo "Display is enabled. Please open a display activity before running any GUI applications."
fi
