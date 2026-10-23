# sing-portal

sing-box 订阅控制台：一个二进制、两个子命令。

- `serve`：控制面。浏览器导入订阅，自动识别格式并转换成 sing-box outbounds，SQLite 存状态，向节点下发配置。
- `agent`：数据面。跑在每台 sing-box 节点上，轮询控制面、写 `outbounds.json`、渲染 config、`sing-box check`、重启、恢复出口选择、回滚。

设计细节和取舍见 [PLAN.md](PLAN.md)。

## 仓库结构

```
crates/core     订阅识别/转换、tag 规范化、revision、config 渲染、Clash API 客户端
crates/store    SQLite（sqlx）+ migrations
crates/server   axum HTTP API、调度器、内嵌前端
crates/agent    节点侧同步/回滚/选择恢复
crates/cli      bin sing-portal（serve / agent）
web             Vue 3 + Vite + TS + Naive UI（Yarn Berry，node-modules 布局）
migrations      SQL schema
fixtures        测试用订阅样例
```

## 构建

前端产物会通过 `rust-embed` 嵌入二进制，所以先构建前端：

```bash
cd web
node .yarn/releases/yarn-4.18.0.cjs install
node .yarn/releases/yarn-4.18.0.cjs build

cd ..
cargo build --release
# 产物：target/release/sing-portal
```

开发前端时用 Vite：

```bash
cd web && node .yarn/releases/yarn-4.18.0.cjs dev   # :5173，/api 代理到 127.0.0.1:3015
cargo run -- serve --static-dir web/dist             # 或直接用内嵌资源
```

## 运行控制面

```bash
sudo install -Dm755 target/release/sing-portal /usr/local/bin/sing-portal
sudo mkdir -p /var/lib/singbox-portal

SING_PORTAL_LISTEN=10.11.0.3:3015 \
SING_PORTAL_DB=/var/lib/singbox-portal/portal.db \
SING_PORTAL_NODES="cola=10.11.0.1:9090,home=10.11.0.3:9090,trunc=10.11.0.4:9090" \
SING_PORTAL_SING_BOX=/run/current-system/sw/bin/sing-box \
sing-portal serve
```

默认只监听 `127.0.0.1:3015`；部署时显式指定内网地址，不要把 3015 暴露到公网。
设置 `SING_PORTAL_SING_BOX` 后，导入和刷新都会先用 `sing-box check` 校验，校验失败不会覆盖旧配置；未设置则跳过校验（响应里 `validated: false`）。

### 环境变量 / 参数

`serve`（CLI 优先于环境变量）：

| 参数 | 环境变量 | 默认 |
|---|---|---|
| `--listen` | `SING_PORTAL_LISTEN` | `127.0.0.1:3015` |
| `--db` | `SING_PORTAL_DB` | `portal.db` |
| `--nodes` | `SING_PORTAL_NODES` | 空（`name=host:port`，逗号分隔） |
| `--sing-box` | `SING_PORTAL_SING_BOX` | 空（跳过校验） |
| `--fetch-proxy` | `SING_PORTAL_FETCH_PROXY` | 空（直连失败时重试用的代理） |
| `--refresh-default` | `SING_PORTAL_REFRESH_DEFAULT` | `6h` |
| `--fetch-timeout` | `SING_PORTAL_FETCH_TIMEOUT` | `20s` |
| `--fetch-max-bytes` | `SING_PORTAL_FETCH_MAX_BYTES` | `8388608` |
| `--static-dir` | `SING_PORTAL_STATIC_DIR` | 空（用内嵌资源） |
| — | `SING_PORTAL_LOG` | `info`（tracing EnvFilter） |

## 运行 agent（每台 sing-box 节点）

先准备一个不含 `outbounds` 的 sing-box 模板（dns/inbounds/route/rule_set 保持现网配置），例如 `/etc/sing-box/template.json`。

```bash
sing-portal agent \
  --portal http://10.11.0.3:3015 \
  --name cola \
  --template /etc/sing-box/template.json \
  --config /run/sing-box/config.json \
  --state-dir /var/lib/sing-box \
  --sing-box /run/current-system/sw/bin/sing-box \
  --clash http://127.0.0.1:9090 \
  --restart-cmd "systemctl restart sing-box"
```

| 参数 | 环境变量 | 默认 |
|---|---|---|
| `--portal` | `SING_PORTAL_URL` | 必填 |
| `--name` | `SING_PORTAL_NODE` | 必填，需与 `--nodes` 中的键一致 |
| `--template` | `SING_PORTAL_TEMPLATE` | 必填 |
| `--config` | `SING_PORTAL_CONFIG` | `/run/sing-box/config.json` |
| `--state-dir` | `SING_PORTAL_STATE_DIR` | `/var/lib/sing-box` |
| `--sing-box` | `SING_PORTAL_SING_BOX` | `sing-box` |
| `--clash` | `SING_PORTAL_CLASH` | `http://127.0.0.1:9090` |
| `--restart-cmd` | `SING_PORTAL_RESTART_CMD` | `systemctl restart sing-box`（空串表示不重启） |
| `--interval` | `SING_PORTAL_INTERVAL` | `15s` |
| `--health-timeout` | `SING_PORTAL_HEALTH_TIMEOUT` | `10s` |
| `--once` | — | 只跑一轮 |
| `--best-effort` | — | 失败也返回 0（供 `ExecStartPre` 用） |

第一次部署、以及 `sing-box.service` 的 `ExecStartPre`：

```bash
sing-portal agent --once --best-effort ...   # 失败也放行，有缓存就用缓存
```

控制面不可达时 agent 不覆盖本地缓存；无缓存时以非零退出（`--best-effort` 下返回 0）。
所有订阅都被停用时控制面下发 `revision: "none"`，agent 跳过应用，继续用旧配置。

## HTTP API

管理接口（无认证，写请求要求 `X-Portal-Request: 1`，无 CORS）：

```
GET    /api/healthz
GET    /api/revision
GET    /api/subscriptions
POST   /api/subscriptions                     {name, url|content, userAgent?, enabled?, autoRefresh?, refreshInterval?}
PATCH  /api/subscriptions/{id}
DELETE /api/subscriptions/{id}
POST   /api/subscriptions/{id}/refresh
GET    /api/nodes
GET    /api/nodes/{name}/proxies
PUT    /api/nodes/{name}/selection            {outbound}   200 已应用 / 202 已排队
POST   /api/nodes/{name}/test                 {tag, url?}  -> {delay}
GET    /api/export                            （含凭据，注意保管）
POST   /api/import
GET    /api/audit?limit=100
```

节点接口：

```
GET  /api/v1/nodes/{name}/state?revision=&selectionRevision=   304 表示无变化
POST /api/v1/nodes/{name}/status                               {revision, selection, health, error}
```

## 支持的订阅格式

- sing-box JSON（`{"outbounds": [...]}` 或 outbounds 数组）
- Clash / Mihomo YAML（`proxies` 子集：ss、vmess、vless、trojan、hysteria2、tuic、anytls、socks5、http）
- base64 编码的 URI 列表、明文 URI 列表（vmess、vless、ss、trojan、hysteria2、tuic、anytls、socks、http）

不支持的协议按单条跳过并返回 warning，不影响整份订阅。

## 测试

```bash
cargo test                      # core / store / server / agent
cd web && node .yarn/releases/yarn-4.18.0.cjs build
```

## 说明

- Yarn 4 的 `nodeLinker` 只有 `pnp` / `pnpm` / `node-modules`，没有 `hoist`；这里用 `node-modules` + `nmHoistingLimits: none`，即传统 hoisting 的 `node_modules` 布局，不使用 PnP。
- 当前没有登录、认证、TLS：只监听内网地址，安全边界在 cryonet。需要公网访问时，在外层加 nginx + 认证，数据模型不用改。
- SQLite 用 WAL；备份直接拷 `portal.db*` 或用 `/api/export`。
