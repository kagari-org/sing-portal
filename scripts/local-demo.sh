#!/usr/bin/env bash
# Local end-to-end run: mock Clash API + portal + one agent sync.
# Everything is throwaway: state lives in .tmp/local and processes are killed on exit.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
work="${WORKDIR:-$root/.tmp/local}"
portal_port="${PORTAL_PORT:-3915}"
clash_port="${CLASH_PORT:-9090}"
binary="${SING_PORTAL_BIN:-$root/target/debug/sing-portal}"
portal="http://127.0.0.1:$portal_port"

if [ ! -x "$binary" ]; then
  echo "missing $binary; build it first: cargo build" >&2
  exit 1
fi

pids=()
cleanup() {
  for pid in "${pids[@]:-}"; do
    kill "$pid" 2>/dev/null || true
  done
}
trap cleanup EXIT

rm -rf "$work"
mkdir -p "$work/run" "$work/state" "$work/tmp"
export TMPDIR="$work/tmp"

json() {
  node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{const v=JSON.parse(s);const f=process.argv[1].split(".");let o=v;for(const k of f)o=o?.[k];console.log(typeof o==="object"?JSON.stringify(o,null,1):o)})' "$1"
}

echo "== start mock clash (:${clash_port}) and portal (:${portal_port})"
MOCK_CLASH_PORT="$clash_port" node "$root/scripts/mock-clash.mjs" >"$work/clash.log" 2>&1 &
pids+=($!)

"$binary" serve \
  --listen "127.0.0.1:$portal_port" \
  --db "$work/portal.db" \
  --nodes "home=127.0.0.1:$clash_port" \
  --sing-box "$root/scripts/fake-sing-box.sh" \
  >"$work/portal.log" 2>&1 &
pids+=($!)

for _ in $(seq 1 50); do
  if curl -fsS "$portal/api/healthz" >/dev/null 2>&1; then break; fi
  sleep 0.1
done
curl -fsS "$portal/api/healthz" >/dev/null

echo
echo "== import fixtures/sub-uri.txt"
node -e 'const fs=require("fs");process.stdout.write(JSON.stringify({name:"Local",content:fs.readFileSync(process.argv[1],"utf8")}))' \
  "$root/fixtures/sub-uri.txt" >"$work/create.json"
curl -fsS -X POST -H 'content-type: application/json' -H 'X-Portal-Request: 1' \
  --data @"$work/create.json" "$portal/api/subscriptions" |
  node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{const j=JSON.parse(s);console.log({format:j.subscription.format,nodes:j.subscription.outboundCount,validated:j.subscription.validated,warnings:j.warnings.length})})'

echo
echo "== GET /api/nodes (Clash API reachable through the mock)"
curl -fsS "$portal/api/nodes" | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{const n=JSON.parse(s)[0];console.log({name:n.name,reachable:n.reachable,version:n.version,selection:n.selection})})'

echo
echo "== switch egress to a concrete outbound"
curl -fsS -X PUT -H 'content-type: application/json' -H 'X-Portal-Request: 1' \
  --data '{"outbound":"local · HY2"}' "$portal/api/nodes/home/selection"
echo

echo "== delay test"
curl -fsS -X POST -H 'content-type: application/json' -H 'X-Portal-Request: 1' \
  --data '{"tag":"local · HY2"}' "$portal/api/nodes/home/test"
echo

echo
echo "== node state, then the same request again (expect 304)"
state="$(curl -fsS "$portal/api/v1/nodes/home/state")"
echo "$state" | json revision
revision="$(echo "$state" | json revision)"
selection_revision="$(echo "$state" | json selectionRevision)"
code="$(curl -s -o /dev/null -w '%{http_code}' "$portal/api/v1/nodes/home/state?revision=$revision&selectionRevision=$selection_revision")"
echo "second request -> HTTP $code"

echo
echo "== run one agent sync against this portal"
"$binary" agent --once \
  --portal "$portal" \
  --name home \
  --template "$root/examples/template.json" \
  --config "$work/run/config.json" \
  --state-dir "$work/state" \
  --sing-box "$root/scripts/fake-sing-box.sh" \
  --clash "http://127.0.0.1:$clash_port" \
  --restart-cmd ""

echo
echo "== files written by the agent"
ls -l "$work/state" "$work/run"
echo "-- config.json outbounds:"
node -e 'const c=JSON.parse(require("fs").readFileSync(process.argv[1],"utf8"));console.log(c.outbounds.map(o=>`${o.type} ${o.tag}`).join("\n"))' "$work/run/config.json"
echo "-- agent state:"
json appliedRevision <"$work/state/state.json"
json appliedSelection <"$work/state/state.json"

echo
echo "== portal view of the node after the sync"
curl -fsS "$portal/api/nodes" | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{const r=JSON.parse(s)[0].report;console.log({revision:r.revision,selection:r.selection,health:r.health,online:r.online})})'

echo
echo "OK. UI: $portal"
if [ "${STAY:-0}" = "1" ]; then
  echo "portal stays up, Ctrl-C stops everything"
  wait
fi
