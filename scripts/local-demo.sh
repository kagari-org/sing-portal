#!/usr/bin/env bash
# Local end-to-end run with a real sing-box: one portal, one agent and two
# sing-box instances (an upstream socks proxy and the agent-managed one).
# Everything is throwaway: state lives in .tmp/local and processes are killed on exit.
# Requires sing-box (SING_BOX_BIN or $PATH) and network access for the bandwidth test.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
work="${WORKDIR:-$root/.tmp/local}"
portal_port="${PORTAL_PORT:-3915}"
socks_port="${SOCKS_PORT:-7898}"
api_port="${API_PORT:-9091}"
binary="${SING_PORTAL_BIN:-$root/target/debug/sing-portal}"
sing_box="${SING_BOX_BIN:-sing-box}"
portal="http://127.0.0.1:$portal_port"

if [ ! -x "$binary" ]; then
  echo "missing $binary; build it first: cargo build" >&2
  exit 1
fi
if ! command -v "$sing_box" >/dev/null 2>&1; then
  echo "missing sing-box; set SING_BOX_BIN" >&2
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
mkdir -p "$work/run" "$work/state" "$work/box" "$work/tmp"
export TMPDIR="$work/tmp"

wait_port() {
  for _ in $(seq 1 100); do
    if node -e 'const net=require("net");const s=net.connect(Number(process.argv[1]),"127.0.0.1");s.on("connect",()=>{s.end();process.exit(0)});s.on("error",()=>process.exit(1))' "$1"; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}

echo "== start an upstream socks proxy (sing-box: socks -> direct)"
cat >"$work/upstream.json" <<EOF
{
  "log": { "level": "warn" },
  "inbounds": [
    { "type": "socks", "tag": "in", "listen": "127.0.0.1", "listen_port": $socks_port }
  ],
  "outbounds": [ { "type": "direct", "tag": "direct" } ]
}
EOF
"$sing_box" run -c "$work/upstream.json" >"$work/upstream.log" 2>&1 &
pids+=($!)
wait_port "$socks_port"

echo
echo "== start the portal"
"$binary" serve \
  --listen "127.0.0.1:$portal_port" \
  --db "$work/portal.db" \
  --auto-candidates 2 \
  >"$work/portal.log" 2>&1 &
pids+=($!)
wait_port "$portal_port"

echo
echo "== import a generated subscription (three socks outbounds)"
node -e '
const tags = ["local-a", "local-b", "local-c"];
const outbounds = tags.map((tag) => ({ type: "socks", tag, server: "127.0.0.1", server_port: Number(process.argv[1]) }));
process.stdout.write(JSON.stringify({ name: "Local", content: JSON.stringify({ outbounds }) }));
' "$socks_port" >"$work/create.json"
curl -fsS -X POST -H 'content-type: application/json' -H 'X-Portal-Request: 1' \
  --data @"$work/create.json" "$portal/api/subscriptions" |
  node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{const j=JSON.parse(s);console.log({format:j.subscription.format,nodes:j.subscription.outboundCount,warnings:j.warnings.length})})'

echo
echo "== start the agent-managed sing-box (it will be reloaded by the agent)"
cat >"$work/run/config.json" <<EOF
{
  "log": { "level": "warn" },
  "services": [
    { "type": "api", "tag": "s_api", "listen": "127.0.0.1", "listen_port": $api_port }
  ],
  "outbounds": [
    { "type": "selector", "tag": "s_out", "outbounds": ["s_direct"], "default": "s_direct" },
    { "type": "direct", "tag": "s_direct" }
  ],
  "route": { "final": "s_out" }
}
EOF
"$sing_box" run -D "$work/box" -C "$work/run" >"$work/box.log" 2>&1 &
box_pid=$!
pids+=($box_pid)
echo "$box_pid" >"$work/box.pid"
wait_port "$api_port"

echo
echo "== start the agent (it dials the portal and registers itself)"
cat >"$work/template.json" <<EOF
{
  "log": { "level": "warn" },
  "services": [
    { "type": "api", "tag": "s_api", "listen": "127.0.0.1", "listen_port": $api_port }
  ],
  "route": { "final": "s_out" }
}
EOF
"$binary" agent \
  --portal "$portal" \
  --name home \
  --template "$work/template.json" \
  --config "$work/run/config.json" \
  --state-dir "$work/state" \
  --api-url "http://127.0.0.1:$api_port" \
  --latency-url "$portal/api/healthz" \
  --test-max-bytes 2097152 \
  --restart-cmd "kill -HUP \$(cat $work/box.pid)" \
  --interval 3s \
  >"$work/agent.log" 2>&1 &
pids+=($!)
for _ in $(seq 1 100); do
  if [ -f "$work/state/outbounds.json" ]; then break; fi
  sleep 0.1
done

echo
echo "== GET /api/providers (nodes grouped by subscription)"
curl -fsS "$portal/api/providers" | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{const p=JSON.parse(s)[0];console.log({provider:p.name,slug:p.slug,nodes:p.nodes.length,first:p.nodes[0].tag})})'

echo
echo "== auto selection: agent probes every node, portal scores and picks"
curl -fsS -X POST -H 'X-Portal-Request: 1' "$portal/api/nodes/home/auto" >/dev/null
for _ in $(seq 1 600); do
  count="$(curl -fsS "$portal/api/nodes/home/measurements" |
    node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>console.log(JSON.parse(s).auto.length))')"
  running="$(curl -fsS "$portal/api/nodes/home/evaluation" |
    node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{const v=JSON.parse(s);console.log(v&&v.running?"yes":"no")})')"
  if [ "$count" -gt 0 ] && [ "$running" = "no" ]; then break; fi
  sleep 0.2
done
curl -fsS "$portal/api/nodes/home/measurements" |
  node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{for(const row of JSON.parse(s).auto.slice(0,5)){console.log((row.best?"* ":"  ")+row.tag.padEnd(12)+" score="+row.score.toFixed(1)+" lat="+(row.latencyMs===null?"n/a":row.latencyMs.toFixed(1))+"ms bw="+(row.downMbps===null?"n/a":row.downMbps.toFixed(1)+"Mbps"))};for(const row of JSON.parse(s).manual){console.log("manual "+row.tag.padEnd(12)+" lat="+(row.latencyMs===null?"n/a":row.latencyMs.toFixed(1))+"ms bw="+(row.downMbps===null?"n/a":row.downMbps.toFixed(1)+"Mbps"))}})'
curl -fsS "$portal/api/nodes" |
  node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{const n=JSON.parse(s)[0];console.log({mode:n.mode,selection:n.selection,score:n.score===null?null:Number(n.score.toFixed(1)),best:n.best})})'

selected="$(curl -fsS "$portal/api/nodes" |
  node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>process.stdout.write(JSON.parse(s)[0].selection))')"

echo
echo "== latency test on the auto-selected node ($selected)"
node -e 'process.stdout.write(JSON.stringify({ tag: process.argv[1], kind: "latency" }))' "$selected" >"$work/payload.json"
curl -fsS -X POST -H 'content-type: application/json' -H 'X-Portal-Request: 1' \
  --data @"$work/payload.json" "$portal/api/nodes/home/test"
echo

echo "== bandwidth test through the test inbound ($selected)"
node -e 'process.stdout.write(JSON.stringify({ tag: process.argv[1], kind: "bandwidth" }))' "$selected" >"$work/payload.json"
curl -sS -X POST -H 'content-type: application/json' -H 'X-Portal-Request: 1' \
  --data @"$work/payload.json" "$portal/api/nodes/home/test" || true
echo

echo
echo "== wait for the agent sync to land"
for _ in $(seq 1 100); do
  if curl -fsS "$portal/api/nodes" | grep -q '"health":"ok"'; then break; fi
  sleep 0.2
done

echo
echo "== files written by the agent"
ls -l "$work/state" "$work/run"
echo "-- config.json outbounds:"
node -e 'const c=JSON.parse(require("fs").readFileSync(process.argv[1],"utf8"));console.log(c.outbounds.map(o=>`${o.type} ${o.tag}`).slice(0,5).join("\n")+"\n...")' "$work/run/config.json"
echo "-- agent state:"
node -e 'const s=JSON.parse(require("fs").readFileSync(process.argv[1],"utf8"));console.log(s.appliedRevision);console.log(s.appliedSelection)' "$work/state/state.json"

echo
echo "== portal view of the machine"
curl -fsS "$portal/api/nodes" | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{const n=JSON.parse(s)[0];console.log({name:n.name,reachable:n.reachable,mode:n.mode,selection:n.selection,health:n.report?.health,revision:n.report?.revision})})'

echo
echo "OK. UI: $portal"
if [ "${STAY:-0}" = "1" ]; then
  echo "services stay up, Ctrl-C stops everything"
  wait
fi
