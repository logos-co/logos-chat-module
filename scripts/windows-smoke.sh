#!/usr/bin/env bash
set -euo pipefail

# Use the exact delivery build pinned by this chat flake, alongside logoscore's
# bundled capability module. All paths are relative to the shared CI stage.
test -f windows-logoscore/modules/capability_module/manifest.json
test ! -e windows-logoscore/modules/delivery_module
test ! -e windows-logoscore/modules/chat_module
cp -R delivery_module-install-portable/modules/delivery_module windows-logoscore/modules/
cp -R install-portable/modules/chat_module windows-logoscore/modules/

CFG=windows-chat-smoke-config
cleanup() {
  run windows-logoscore/bin/logoscore.exe --config-dir "$CFG" stop >/dev/null 2>&1 || true
}
trap cleanup EXIT

./windows-logoscore/bin/logoscore.exe -D -m ./windows-logoscore/modules \
  --config-dir "./$CFG" > windows-chat-smoke-daemon.log 2>&1 &

ready=0
for attempt in $(seq 1 30); do
  if run windows-logoscore/bin/logoscore.exe --config-dir "./$CFG" status |
    grep -qF '"status":"running"'; then
    ready=1
    break
  fi
  sleep 1
done
if [ "$ready" -ne 1 ]; then
  cat windows-chat-smoke-daemon.log
  exit 1
fi

run windows-logoscore/bin/logoscore.exe --config-dir "./$CFG" load-module chat_module |
  tee windows-chat-smoke-load.json
grep -qF '"module":"chat_module"' windows-chat-smoke-load.json
grep -qF '"delivery_module"' windows-chat-smoke-load.json

run windows-logoscore/bin/logoscore.exe --config-dir "./$CFG" module-info chat_module |
  tee windows-chat-smoke-info.json
grep -qF '"name":"status"' windows-chat-smoke-info.json

run windows-logoscore/bin/logoscore.exe --config-dir "./$CFG" call chat_module status |
  tee windows-chat-smoke-status.json
grep -qF '"delivery_state":"stopped"' windows-chat-smoke-status.json

run windows-logoscore/bin/logoscore.exe --config-dir "./$CFG" stop
trap - EXIT
