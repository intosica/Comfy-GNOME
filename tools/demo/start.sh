#!/bin/sh
# Restart the demo stack: Xvfb :96 (1280x800), mock backend on 8198, the app on :96.
# Anchored patterns so this never kills the shell that runs it.
cd "$(dirname "$0")/../.." || exit 1
for pat in '^python3 tools/demo/mock_backend' '^Xvfb :96' '^\./target/debug/comfy-gnome$'; do
    pids=$(pgrep -f "$pat" 2>/dev/null || true)
    [ -n "$pids" ] && kill $pids 2>/dev/null
done
sleep 2
mkdir -p /tmp/demo-config/comfy-gnome /tmp/demo-cache .shots
printf '[launch]\nscript=\ncomfy_dir=\nargs=\nhost=127.0.0.1\nport=8198\nsuppress_browser=true\n' > /tmp/demo-config/comfy-gnome/launch.ini
Xvfb :96 -screen 0 1280x800x24 >/dev/null 2>&1 &
sleep 1
python3 tools/demo/mock_backend.py --port 8198 --preview >/tmp/mock.log 2>&1 &
sleep 1
# A private D-Bus-less run: otherwise a running instance (the user's) swallows this one via
# GApplication uniqueness and this app exits before showing a window.
DISPLAY=:96 DBUS_SESSION_BUS_ADDRESS=disabled: XDG_CONFIG_HOME=/tmp/demo-config XDG_CACHE_HOME=/tmp/demo-cache ./target/debug/comfy-gnome >/tmp/demo.log 2>&1 &
sleep 6
DISPLAY=:96 xdotool mousemove 546 761 click 1   # Skip — connect to the running backend
sleep 9
echo "demo stack up on :96 (mock backend 8198)"
