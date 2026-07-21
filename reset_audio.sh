#!/usr/bin/env bash
# Resets PipeWire/WirePlumber to a clean, standard state before launching
# FerroMix — clears any lingering "default" sink/source overrides and stray
# link state from earlier FerroMix sessions (older builds set system default
# sink/source directly; current builds no longer do, but that metadata
# persists in WirePlumber across FerroMix runs since it's not owned by us).
# Safe to run any time: it's the same reset a normal `systemctl --user
# restart` of these services would do, nothing FerroMix-specific.
set -e

echo "Stopping FerroMix (if running)..."
pkill -f 'target/(debug|release)/ferromix2-daemon' 2>/dev/null || true
pkill -f 'target/(debug|release)/ferromix2$' 2>/dev/null || true
sleep 1

echo "Restarting PipeWire + WirePlumber user services..."
systemctl --user restart pipewire.socket pipewire-pulse.socket
systemctl --user restart wireplumber.service

echo "Waiting for the graph to come back up..."
sleep 2
if ! pw-cli info 0 >/dev/null 2>&1; then
    echo "WARNING: pipewire doesn't look up yet — give it a few more seconds." >&2
fi

echo "Done. Audio is back to standard WirePlumber routing."
echo "Start FerroMix normally now."
