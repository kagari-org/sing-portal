#!/usr/bin/env bash
# Local end-to-end run: mock Clash API + mock proxy inbound + portal + one agent.
# Everything is throwaway: state lives in .tmp/local and processes are killed on exit.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
work="${WORKDIR:-$root/.tmp/local}"
portal_port="${PORTAL_PORT:-3915}"
clash_port="${CLASH_PORT:-9090}"
test_port="${TEST_PORT:-3016}"
proxy_port="${PROXY_PORT:-7898}"
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

echo "== start mocks and services"
MOCK_CLASH_PORT="$clash_port" node "$root/scripts/mock-clash.mjs" >"$work/clash.log" 2>&1 &
pids+=($!)

MOCK_PROXY_PORT="$proxy_port" node "$root/scripts/mock-proxy.mjs" >"$work/proxy.log" 2>&1 &
pids+=($!)

"$binary" serve \
  --listen "127.0.0.1:$portal_port" \
  --db "$work/portal.db" \
  --nodes "home=127.0.0.1:$clash_port" \
  --sing-box "$root/scripts/fake-sing-box.sh" \
  --test-port "$test_port" \
  >"$work/portal.log" 2>&1 &
pids+=($!)



for _ in $(seq 1 50); do
  if curl -fsS "$portal/api/healthz" >/dev/null 2>&1; then break; fi
  sleep 0.1
done
curl -fsS "$portal/api/healthz" >/dev/null

"$binary" agent \
  --portal "$portal" \
  --name home \
  --template "$root/examples/template.json" \
  --config "$work/run/config.json" \
  --state-dir "$work/state" \
  --sing-box "$root/scripts/fake-sing-box.sh" \
  --clash "http://127.0.0.1:$clash_port" \
  --restart-cmd "" \
  --interval 3s \
  --test-listen "127.0.0.1:$test_port" \
  --proxy-inbound "http://127.0.0.1:$proxy_port" \
  --test-latency-url "http://probe.invalid/" \
  --test-download-url "http://speed.invalid/payload" \
  >"$work/agent.log" 2>&1 &
pids+=($!)
for _ in $(seq 1 100); do
  if [ -f "$work/state/outbounds.json" ]; then break; fi
  sleep 0.1
done

echo
echo "== import fixtures/sub-uri.txt"
node -e 'const fs=require("fs");process.stdout.write(JSON.stringify({name:"Provider A",content:fs.readFileSync(process.argv[1],"utf8")}))' \
  "$root/fixtures/sub-uri.txt" >"$work/create.json"
curl -fsS -X POST -H 'content-type: application/json' -H 'X-Portal-Request: 1' \
  --data @"$work/create.json" "$portal/api/subscriptions" |
  node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{const j=JSON.parse(s);console.log({format:j.subscription.format,nodes:j.subscription.outboundCount,validated:j.subscription.validated,warnings:j.warnings.length})})'

echo
echo "== GET /api/providers (nodes grouped by subscription)"
curl -fsS "$portal/api/providers" | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{const p=JSON.parse(s)[0];console.log({provider:p.name,slug:p.slug,nodes:p.nodes.length,first:p.nodes[0].tag})})'

echo
echo "== auto selection: agent probes every node, portal scores and switches"
curl -fsS -X POST -H 'X-Portal-Request: 1' "$portal/api/nodes/home/auto" >/dev/null
for _ in $(seq 1 100); do
  count="$(curl -fsS "$portal/api/nodes/home/measurements" |
    node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>console.log(JSON.parse(s).length))')"
  if [ "$count" -gt 0 ]; then break; fi
  sleep 0.2
done
curl -fsS "$portal/api/nodes/home/measurements" |
  node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{for(const row of JSON.parse(s).slice(0,5)){console.log((row.best?"* ":"  ")+row.tag.padEnd(28)+" score="+row.score.toFixed(1)+" lat="+(row.latencyMs===null?"n/a":row.latencyMs.toFixed(1))+"ms bw="+(row.downMbps===null?"n/a":row.downMbps.toFixed(1)+"Mbps"))}})'
curl -fsS "$portal/api/nodes" |
  node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{const n=JSON.parse(s)[0];console.log({mode:n.mode,selection:n.selection,score:n.score===null?null:Number(n.score.toFixed(1)),best:n.best})})'

echo
echo "== switch egress to a concrete node (this pins the machine to manual)"
curl -fsS -X PUT -H 'content-type: application/json' -H 'X-Portal-Request: 1' \
  --data '{"outbound":"provider-a · HY2"}' "$portal/api/nodes/home/selection"
echo

echo
echo "== latency test (runs on the agent, through its proxy inbound)"
curl -fsS -X POST -H 'content-type: application/json' -H 'X-Portal-Request: 1' \
  --data '{"tag":"provider-a · HY2","kind":"latency"}' "$portal/api/nodes/home/test"
echo

echo "== bandwidth test (runs on the agent, through its proxy inbound)"
curl -fsS -X POST -H 'content-type: application/json' -H 'X-Portal-Request: 1' \
  --data '{"tag":"provider-a · HY2","kind":"bandwidth"}' "$portal/api/nodes/home/test"
echo

echo
echo "== node state, then the same request again (expect 304)"
state="$(curl -fsS "$portal/api/v1/nodes/home/state")"
revision="$(echo "$state" | json revision)"
selection_revision="$(echo "$state" | json selectionRevision)"
code="$(curl -s -o /dev/null -w '%{http_code}' "$portal/api/v1/nodes/home/state?revision=$revision&selectionRevision=$selection_revision")"
echo "revision: $revision"
echo "second request -> HTTP $code"

echo
echo "== wait for the agent sync to land"
for _ in $(seq 1 50); do
  if curl -fsS "$portal/api/nodes" | grep -q '"health":"ok"'; then break; fi
  sleep 0.2
done

echo
echo "== files written by the agent"
ls -l "$work/state" "$work/run"
echo "-- config.json outbounds:"
node -e 'const c=JSON.parse(require("fs").readFileSync(process.argv[1],"utf8"));console.log(c.outbounds.slice(0,4).map(o=>`${o.type} ${o.tag}`).join("\n")+"\n...")' "$work/run/config.json"
echo "-- agent state:"
json appliedRevision <"$work/state/state.json"
json appliedSelection <"$work/state/state.json"

echo
echo "== portal view of the machine"
curl -fsS "$portal/api/nodes" | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{const n=JSON.parse(s)[0];console.log({name:n.name,reachable:n.reachable,selection:n.selection,health:n.report?.health,revision:n.report?.revision})})'

echo
echo "OK. UI: $portal"
if [ "${STAY:-0}" = "1" ]; then
  echo "services stay up, Ctrl-C stops everything"
  wait
fi
