# sing-portal 实施方案

独立仓库 `kagari/sing-portal`：一个二进制，两个子命令。`serve` 跑内网控制面（订阅管理 + 配置下发），`agent` 跑在每个 sing-box 节点上（拉配置、渲染、重启、恢复出口选择）。

本仓库**不含** NixOS 模块、sops、systemd unit。部署相关的东西在 `nix/infra` 里，这里只产出可执行文件。

## 0. 与原计划（infra 内 Go 版）的差异

| 项 | 原计划 | 本方案 |
|---|---|---|
| 语言 | Go + 标准库 | Rust（edition 2024） |
| 存储 | 单个 `state.json` 原子写 | SQLite（WAL），`sqlx` 管理 migration |
| 格式转换 | `sub2sing-box` / `clash2singbox` 两个 Go 二进制 | 全部原生 Rust 实现，无外部进程 |
| 前端 | 手写静态文件 | Vue 3 + Vite + TS + Naive UI，构建产物 `rust-embed` 进二进制 |
| 包管理 | — | Yarn Berry 4 + `nodeLinker: node-modules`（Yarn 4 没有 `hoist` 这个值，node-modules 本身就是 hoisting 布局），`yarn.lock` 入库 |
| 配置 | Nix 生成 | 纯环境变量 + 命令行参数，无配置文件 |
| 认证 | 无（内网） | 无（内网），仅保留 `X-Portal-Request` 防跨站写 |
| 迁移 fallback | 从 sops 旧 secret 引导 | 不做；由 infra 的 `ExecStartPre` 用 `-` 前缀容忍失败 |

## 1. 目标与成功标准

目标：订阅节点列表不再进 Git/Nix。浏览器里导入、启停订阅，节点自动跟随，按机器切换出口。

成功标准：

1. 导入 sing-box JSON、Clash YAML、base64 URI 列表、明文 URI 列表都能自动识别并转换；坏数据被拒绝且旧配置不变。
2. 启用/停用/刷新订阅后，在线节点在一个轮询周期内更新；离线节点恢复后自动追上。
3. 每台机器的出口选择（`s_auto` / `s_direct` / 具体节点）重启后仍生效。
4. 节点启动不依赖控制面：`agent sync --once --best-effort` 在 portal 不可达时用本地缓存，无缓存则非零退出（infra 用 `-` 前缀忽略）。
5. 新配置在重启前过 `sing-box check`；重启后 10s 内 Clash API 不可用则回滚上一份 outbounds。
6. `serve` 默认只监听 `127.0.0.1:3015`，部署时显式设 `SING_PORTAL_LISTEN=10.11.0.3:3015`。
7. 单二进制：`sing-portal serve` / `sing-portal agent ...`，前端资源内嵌。

## 2. 仓库结构

```
Cargo.toml                 # workspace
Cargo.lock                 # 入库
rust-toolchain.toml        # stable + rustfmt/clippy
rustfmt.toml
flake.nix                  # 仅 devShell：rustc/cargo/clippy/rustfmt/nodejs/yarn
crates/
  cli/                     # [[bin]] sing-portal：解析子命令、装载配置、跑 serve/agent
  core/                    # 检测、转换、规范化、revision、renderer、Clash 客户端、错误类型
  store/                   # sqlx pool、migrations、订阅/选择/上报/审计的 CRUD
  server/                  # axum router、handler、调度器、静态资源
  agent/                   # 同步循环、缓存写入、重启与回滚
migrations/
  0001_init.sql
web/
  package.json             # packageManager: yarn@4.x
  .yarnrc.yml              # nodeLinker: node-modules (hoisting)
  vite.config.ts           # dev proxy /api -> 127.0.0.1:3015
  src/ (Vue 3 + TS + Naive UI + vue-router + pinia)
  dist/                    # 构建产物，不入库
fixtures/
  sub-singbox.json
  sub-clash.yaml
  sub-uri.txt
  sub-uri.base64.txt
  url-schemes.txt
docs/
  README.md                # 用法、env 全表、部署、回滚、备份
```

## 3. 依赖选型（实现时 `cargo add` 定型，Cargo.lock 锁版本）

- 运行时/异步：`tokio`（rt-multi-thread, signal, time, process）
- HTTP 服务：`axum` 0.8、`tower`、`tower-http`（trace、限流可选）
- HTTP 客户端：`reqwest`（rustls-tls, gzip, brotli, 无默认 native-tls）
- DB：`sqlx`（runtime-tokio, sqlite, migrate；用运行时查询，不用 `query!` 宏）
- 序列化：`serde`、`serde_json`；YAML 用 `serde_yaml_ng`（`serde_yaml` 已弃用）
- 编码/哈希：`sha2`、`hex`、`base64`、`url`、`percent-encoding`
- CLI/配置：`clap`（derive + env）
- 日志：`tracing`、`tracing-subscriber`（env-filter）
- ID：`uuid`（v7）；时间：`time`（格式化 RFC3339）
- 错误：`thiserror`（库）、`anyhow`（bin）
- 静态资源：`rust-embed`（feature `embed-web`，默认开启）
- 测试：`tempfile`、`wiremock`（agent 对 portal/Clash 的 mock）

## 4. 架构与数据流

```
内网浏览器 ──► http://10.11.0.3:3015  serve
                        │  SQLite portal.db
                        │  订阅抓取 → 检测 → 转换 → 规范化 → sing-box check
                        │  Clash API 客户端（读 /proxies、PUT /proxies/s_out、delay）
                        ▼
            GET /api/v1/nodes/{name}/state        (轮询，15s)
            POST /api/v1/nodes/{name}/status      (上报)
                        │
      ┌─────────────────┼─────────────────┐
   cola              home              trunc
   agent             agent             agent
   outbounds.json    ...               ...
   config.json → sing-box             
```

原则：

- 控制面不转发流量；数据面自持缓存。
- 订阅只在一处转换（`serve`），节点只拿到标准 outbounds。
- 出口切换走 Clash API，不重启；只有 outbounds revision 变化才重启 sing-box。

## 5. 数据模型（`migrations/0001_init.sql`）

```sql
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE subscriptions (
  id                   TEXT PRIMARY KEY,
  name                 TEXT NOT NULL,
  slug                 TEXT NOT NULL UNIQUE,
  source               TEXT NOT NULL CHECK (source IN ('url','manual')),
  url                  TEXT,
  user_agent           TEXT NOT NULL DEFAULT '',
  format               TEXT NOT NULL,            -- sing-box-json|clash-yaml|uri|uri-base64
  content              TEXT NOT NULL,            -- 最近一次原始内容（可能含凭据，不写日志）
  outbounds            TEXT NOT NULL,            -- JSON 数组
  enabled              INTEGER NOT NULL DEFAULT 1,
  auto_refresh         INTEGER NOT NULL DEFAULT 1,
  refresh_interval_secs INTEGER NOT NULL DEFAULT 21600,
  next_refresh_at      INTEGER,
  last_fetched_at      INTEGER,
  last_error           TEXT NOT NULL DEFAULT '',
  created_at           INTEGER NOT NULL,
  updated_at           INTEGER NOT NULL
);

CREATE TABLE selections (
  node_name   TEXT PRIMARY KEY,
  outbound    TEXT NOT NULL,
  updated_at  INTEGER NOT NULL
);

CREATE TABLE node_reports (
  node_name   TEXT PRIMARY KEY,
  revision    TEXT NOT NULL DEFAULT '',
  selection   TEXT NOT NULL DEFAULT '',
  health      TEXT NOT NULL DEFAULT '',   -- ok|degraded|no_active_subscriptions
  error       TEXT NOT NULL DEFAULT '',
  reported_at INTEGER NOT NULL
);

CREATE TABLE audit_log (
  id     INTEGER PRIMARY KEY AUTOINCREMENT,
  at     INTEGER NOT NULL,
  actor  TEXT NOT NULL,             -- ui | agent:<name>
  action TEXT NOT NULL,
  target TEXT NOT NULL DEFAULT '',
  detail TEXT NOT NULL DEFAULT ''   -- 不含 URL token / 凭据
);

CREATE INDEX idx_subscriptions_next_refresh ON subscriptions(auto_refresh, next_refresh_at);
```

要点：

- `outbounds` 与 `content` 分开存：转换结果用于下发，原始内容用于重新解析和排错。
- `slug` 创建时生成、永不变：小写 ASCII/数字/`-`，最长 16，冲突加 `-2`。
- `node_reports` 落库（不是纯内存），重启后 UI 仍能看到最后一次上报；在线判定用 `reported_at` 距今 < 3×interval。
- `revision` 不落库，按启用订阅的 outbounds 现算（见 7.4）。
- 选择变更后立刻尝试推给节点；失败只记审计，等 agent 下一轮自己对齐。

## 6. HTTP API

管理接口（无认证，写请求要求 `X-Portal-Request: 1`，无 CORS）：

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/healthz` | 存活 + DB 可读 |
| GET | `/api/subscriptions` | 列表；`url` 掩码为 `scheme://host/…`，只留 host |
| POST | `/api/subscriptions` | `{name, url? , content?, userAgent?, enabled?, autoRefresh?, refreshInterval?}`；创建即抓取/转换/校验，失败返回 422 且不落库 |
| PATCH | `/api/subscriptions/{id}` | 改名、启停、换 URL/UA、开关自动刷新、改周期；URL 变化触发刷新 |
| DELETE | `/api/subscriptions/{id}` | 删除；相关选择回退到 `s_auto` 并记审计 |
| POST | `/api/subscriptions/{id}/refresh` | 立即抓取；失败保留旧 outbounds，只写 `last_error` |
| GET | `/api/revision` | 当前 revision + 节点数 + 启用订阅数 |
| GET | `/api/nodes` | 配置中的节点 + 最近上报 + 期望选择 + 实时状态（并发查 Clash `/version`，1s 超时） |
| GET | `/api/nodes/{name}/proxies` | 透传 Clash `/proxies`（UI 展示实时状态） |
| PUT | `/api/nodes/{name}/selection` | `{outbound}`；tag 必须存在；调 `PUT /proxies/s_out`，成功 200，节点不可达 202（已存期望值） |
| POST | `/api/nodes/{name}/test` | `{tag, url?}`；调 `/proxies/{tag}/delay`，返回毫秒或错误 |
| GET | `/api/export` | 导出 JSON（含敏感信息，`Content-Disposition: attachment`） |
| POST | `/api/import` | 导入并校验后整体替换；版本号不匹配返回 400 |
| GET | `/api/audit?limit=100` | 最近审计 |

节点协议（无认证，内网）：

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/v1/nodes/{name}/state?revision=&selectionRevision=` | 返回 `{revision, selectionRevision, selection, outbounds}`；两个 revision 都匹配返回 304 |
| POST | `/api/v1/nodes/{name}/status` | `{revision, selection, health, error}`，落 `node_reports` + 审计（不刷屏，只在变化时记） |

约定：

- `revision == "none"` 时 `outbounds: []`，agent 跳过应用。
- 未知节点名 → 404（节点列表来自 `SING_PORTAL_NODES`）。
- 错误体统一 `{"error": "...", "detail": "..."}`；2xx 只返回 JSON。
- 未命中静态文件时回退 `index.html`（SPA），但 `/api/*` 不参与回退。

## 7. 订阅检测、转换与规范化

### 7.1 抓取（`serve` 侧，`source=url` 时）

- 只允许 `http`/`https`；超时 20s；响应体上限 8 MiB；重定向最多 3 次。
- 默认 UA `sing-box/1.14.0`，可被订阅级 `user_agent` 覆盖。
- 直连失败且设置了 `SING_PORTAL_FETCH_PROXY` 时，经该代理重试一次。
- `Content-Type: text/html` 或 body 以 `<` 开头 → 报“订阅可能过期/需要登录”，保留旧数据。
- 手动导入（`source=manual`）跳过抓取。

### 7.2 检测顺序

1. 去 BOM 与首尾空白；空内容 → 错误。
2. 尝试 JSON：对象含 `outbounds` → `sing-box-json`；对象含 `proxies` → 按 YAML 处理；数组且元素含 `type`+`tag` → 当作 outbounds 数组。
3. 行首出现 `proxies:` 或 `proxy-groups:` → `clash-yaml`。
4. base64 解码（标准/URL-safe，容忍换行）后含 `://` → `uri-base64`。
5. 含已知 scheme（`vmess://` 等）→ `uri`。
6. 其他 → 422 `unrecognized subscription format`。

### 7.3 原生转换支持矩阵（v1）

| 来源 | 支持 |
|---|---|
| sing-box JSON | `outbounds` 数组直接取；过滤 `selector`/`urltest`/`direct`/`block`/`dns` |
| URI 列表 | `vmess://`（base64 JSON 与旧版 query 两种）、`vless://`、`ss://`（SIP002 与整体 base64）、`trojan://`、`hysteria2://`（含 `hy2://`）、`tuic://`、`anytls://`、`socks://`/`socks5://`、`http(s)://` |
| Clash YAML | `proxies[]` 中 `ss`/`vmess`/`vless`/`trojan`/`hysteria2`/`tuic`/`anytls`/`socks5`/`http`；`proxy-groups`、`rules`、其余顶层键忽略 |

明确不支持（v1）：`ssh`、`wireguard`、`hysteria`(v1)、`naive`、`juicity`、`mieru`；遇到时按“单条跳过 + warning”处理，不让整份订阅失败。

URI 解析细节：

- `vless`：把 `type=tcp|ws|grpc|http`、`security=tls|reality|none`、`sni`、`alpn`、`flow`、`fp`、`pbk`、`sid`、`path`、`host`、`serviceName` 映射到 `transport`/`tls` 结构。
- `vmess`：base64 JSON 的 `aid`/`scy`/`net`/`tls`/`host`/`path`/`sni`/`alpn`/`fp` 映射；`aid` 缺省 0。
- `ss`：`method:password@host:port` 与 SIP002 的 `?plugin=`（`obfs-local`/`v2ray-plugin` 基础参数）映射；其余插件按 warning 跳过。
- `hysteria2`/`tuic`：`insecure=1` → `tls.insecure`；`sni`、`obfs`（hysteria2 salamander）、`congestion_control`、`udp_relay_mode`。
- 所有 URI：ignored 未知参数，但在 `warnings` 里返回一次汇总（去重，不回显密码）。

### 7.4 规范化与 revision

- 每个节点必须有 `type` 和 `tag`；缺失则丢弃。
- tag 加前缀：`"<slug> · <原 tag>"`（分隔符固定为 ` · `）。
- 同一订阅内重名：保留第一个，后续加 `#2`、`#3`。
- 按 tag 升序排序；单订阅上限 1000 个 outbound，超出报错。
- `revision = "sha256:" + sha256(canonical_json(所有启用订阅的 outbounds，按 tag 排序))`。
- 全部停用 → `revision = "none"`（不计算哈希）。
- `selectionRevision = "sha256:" + sha256(node + "\\0" + selection)`，按节点单独计算；选择不变则哈希不变。

### 7.5 校验

- 设置 `SING_PORTAL_SING_BOX` 时：构造 `{log, outbounds: [s_out, s_auto, s_direct, ...nodes]}` 写临时文件（0600），跑 `sing-box check -c`，失败 → 拒绝导入/刷新，返回 stderr 摘要（去绝对路径）。
- 未设置时跳过，日志打一次 warning，API 响应里 `validated: false`。
- 校验不过的订阅**不落库**（新建）或**保留旧 outbounds**（刷新）。

## 8. Renderer 与 Agent

### 8.1 Renderer（`core`，agent 调用）

输入：模板 JSON（无 `outbounds` 键）、`outbounds.json`。

输出 `config.json`：

```json
"outbounds": [
  {"type":"selector","tag":"s_out","outbounds":["s_auto","s_direct","…节点"],"default":"s_auto"},
  {"type":"urltest","tag":"s_auto","outbounds":["…节点"],"url":"https://www.gstatic.com/generate_204","interval":"5m","tolerance":50},
  {"type":"direct","tag":"s_direct"},
  "…订阅 outbounds"
]
```

- 无节点：`s_out.outbounds=["s_direct"]`，`default="s_direct"`，不生成 `s_auto`。
- 模板中若残留 `outbounds` 键 → 直接报错，避免静默覆盖。
- 注入后 json 序列化缩进 2，键序保持 serde 结构顺序（稳定输出，便于 diff）。

### 8.2 Agent 同步流程

```
agent sync（常驻，默认 15s；--once 跑一轮）
1. 读本地 state.json（applied_revision、applied_selection）
2. GET /api/v1/nodes/<name>/state?revision=&selectionRevision=
   304 → 跳到 6
3. revision=="none" 或 outbounds 为空 → 不覆盖缓存，上报 no_active_subscriptions，停
4. 写 outbounds.next.json → 渲染 config.next.json → sing-box check
   失败 → 删除临时文件，上报 error，缓存不变
5. 成功：备份 outbounds.json → outbounds.prev.json；原子 rename 两份文件；
   按 --restart-cmd 重启（空则不重启；ExecStartPre 场景传空）；
   轮询 http://<node-addr>:9090/version 最多 10s
   失败 → 恢复 prev、重新渲染、再重启一次、上报 rollback
6. 应用选择：GET /proxies/s_out，与期望不同则 PUT；期望 tag 不在当前 outbounds 里 →
   回退 s_auto，其次 s_direct，并在上报里带 warning
7. POST /api/v1/nodes/<name>/status
```

`--best-effort`：任何步骤失败只记日志并返回 0（供 `ExecStartPre` 使用）；不带该参数时失败返回非 0。

### 8.3 Agent 状态文件（目录 `--state-dir`，默认 `/var/lib/sing-box`）

- `outbounds.json`、`outbounds.prev.json`、`outbounds.next.json`
- `state.json`：`{applied_revision, applied_selection, last_error, updated_at}`
- `config.json` / `config.next.json`（输出路径 `--config`，默认 `/run/sing-box/config.json`）
- 写入一律 `0600`，rename 前 `fsync`。

## 9. 前端

页面（Vue Router）：

- `/`：概览——revision、启用订阅数、各节点在线/期望出口/最近上报。
- `/subscriptions`：表格（名称、slug、格式、节点数、启用、上次刷新、错误）；新建/编辑抽屉（URL 或粘贴内容、UA、自动刷新、周期）；行内启停、刷新、删除（二次确认）。
- `/nodes`：表格（节点、在线、期望出口、实际出口、上报 revision、健康）；下拉切换出口；单节点“测延迟”按钮；错误展开。
- `/settings`：导出（下载 JSON，明确警告含凭据）、导入（文件选择 + 校验预览）。

工程：

- Yarn Berry：`.yarnrc.yml` 设 `nodeLinker: node-modules`（`nmHoistingLimits: none`，即传统 hoisting 布局），`packageManager: "yarn@4.18.0"`，release 文件 `.yarn/releases/yarn-4.18.0.cjs` 入库，`yarn.lock` 入库，`node_modules` 不入库。
- Vite + `vue-tsc`；构建产物 `web/dist`，release 构建由 `rust-embed` 收进二进制。
- dev：`yarn dev`（Vite :5173，proxy `/api` → `127.0.0.1:3015`），`cargo run -- serve --no-embed`（读 `SING_PORTAL_STATIC_DIR`）。
- 无 UI 框架以外的样式库；Naive UI 主题用 `darkTheme` 跟随系统。
- 请求封装统一加 `X-Portal-Request: 1`、错误 toast。

## 10. 配置：环境变量 + 命令行参数

前缀 `SING_PORTAL_`。优先级：CLI > env > 默认值。

`serve`：

| 参数 / env | 默认 | 说明 |
|---|---|---|
| `--listen` / `SING_PORTAL_LISTEN` | `127.0.0.1:3015` | 部署时设 `10.11.0.3:3015` |
| `--db` / `SING_PORTAL_DB` | `./portal.db` | 父目录需存在；库文件自动创建 |
| `--nodes` / `SING_PORTAL_NODES` | 空 | `cola=10.11.0.1:9090,home=10.11.0.3:9090`；空则节点页报“未配置” |
| `--sing-box` / `SING_PORTAL_SING_BOX` | 空 | 用于导入校验；空则跳过 |
| `--fetch-proxy` / `SING_PORTAL_FETCH_PROXY` | 空 | 例如 `http://127.0.0.1:7897` |
| `--refresh-default` / `SING_PORTAL_REFRESH_DEFAULT` | `6h` | 新订阅默认周期 |
| `--fetch-timeout` / `SING_PORTAL_FETCH_TIMEOUT` | `20s` | |
| `--fetch-max-bytes` / `SING_PORTAL_FETCH_MAX_BYTES` | `8388608` | |
| `--static-dir` / `SING_PORTAL_STATIC_DIR` | 空 | 覆盖内嵌资源 |
| `--log` / `SING_PORTAL_LOG` | `info` | tracing EnvFilter |

`agent`：

| 参数 / env | 默认 | 说明 |
|---|---|---|
| `--portal` / `SING_PORTAL_URL` | 必填 | `http://10.11.0.3:3015` |
| `--name` / `SING_PORTAL_NODE` | 必填 | 节点名，需与 `--nodes` 里的键一致 |
| `--template` / `SING_PORTAL_TEMPLATE` | 必填 | sing-box 模板（无 outbounds 键） |
| `--config` / `SING_PORTAL_CONFIG` | `/run/sing-box/config.json` | 渲染输出 |
| `--state-dir` / `SING_PORTAL_STATE_DIR` | `/var/lib/sing-box` | 缓存与状态 |
| `--sing-box` / `SING_PORTAL_SING_BOX` | `sing-box` | PATH 查找 |
| `--clash` / `SING_PORTAL_CLASH` | `http://127.0.0.1:9090` | 本机 Clash API |
| `--restart-cmd` / `SING_PORTAL_RESTART_CMD` | `systemctl restart sing-box` | 空字符串 = 不重启 |
| `--interval` / `SING_PORTAL_INTERVAL` | `15s` | 轮询间隔 |
| `--once` | — | 只跑一轮，不带 `--best-effort` 时失败返回非 0 |
| `--best-effort` | — | 失败也返回 0 |

## 11. 错误处理与边界

- 任何写入（state.json、outbounds.json、config.json）都是“临时文件 + fsync + rename”。
- 订阅抓取/转换失败：新建拒绝、刷新保留旧值，只更新 `last_error` 与 `next_refresh_at`（指数退避 5m/15m/1h/6h，成功重置）。
- 自动刷新调度：单 tokio 循环，每 5s 扫一次到期订阅；同订阅并发刷新用 `Mutex` 去重；启动时把过期订阅的 `next_refresh_at` 立即置为 now。
- 节点不可达：`/api/nodes` 不 500，逐节点标记 `reachable:false`；选择接口返回 202 并存期望值。
- 删除订阅/节点 tag 消失：agent 侧回退 `s_auto` → `s_direct` 并上报；portal 侧删除订阅时把引用它的选择改成 `s_auto`。
- 请求体上限 16 MiB（手动粘贴订阅可能较大）；JSON 解析失败返回 400 而不是 panic。
- 路径穿越：静态资源只从内嵌数据或 `--static-dir` 根下取，拒绝 `..`。
- 日志不打印订阅内容、URL query、节点密码；审计只记动作与截断后的 host。

## 12. 测试

单元（`core`）：

- 检测：sing-box JSON、Clash YAML、base64、明文 URI、HTML、空、超限。
- URI：每种 scheme 一条 fixture（含带/不带 TLS、ws/grpc、reality、hy2 obfs），断言结构字段。
- Clash YAML：混合协议 + `proxy-groups`，断言组被忽略。
- 规范化：前缀、去重、排序、超限、过滤非节点类型。
- revision：同内容稳定、启停变化、全停用为 `none`、选择哈希稳定。
- renderer：有/无节点、模板残留 outbounds 报错、稳定输出。

集成：

- `store`：内存 SQLite 跑 migrations + CRUD + 唯一约束冲突。
- `server`：`tower::ServiceExt::oneshot` 打 router——创建订阅（mock 抓取）、304、`X-Portal-Request` 缺失被拒、未知节点 404、导出/导入往返。
- `agent`：`wiremock` 模拟 portal 与 Clash API，覆盖应用成功、check 失败不覆盖、重启后健康检查失败回滚、期望 tag 不存在回退。

人工验收（infra 部署后）：

- 断开 portal，节点重启仍能起；portal 恢复后一个周期内追上。
- 导入坏配置被 `sing-box check` 拒绝，节点配置不变。
- 全部订阅停用后节点仍可用（不推空集合）。
- `ss -ltnp` 只看到 `10.11.0.x:9090` 和 `10.11.0.3:3015`。

## 13. 实施阶段

- **P1 后端骨架**：workspace、`core`（检测/转换/规范化/revision/renderer/Clash 客户端）、`store`、`server`（API + 调度器）、`cargo test` 全绿、`fixtures` 齐备。
- **P2 前端**：Vite + Vue 3 + Naive UI 四个页面、`rust-embed` 打包、`--static-dir` 开发模式验证。
- **P3 agent**：同步/渲染/校验/重启/回滚/选择恢复 + mock 测试；`sing-box check` 真实二进制集成（有则跑，无则 skip）。
- **P4 收尾**：导出/导入、审计查询、README（含 infra 侧探针配置示例，不含 NixOS 代码）。

## 14. 假设与不做

假设：

- 部署在 Linux x86_64；sing-box 1.14.x；节点与 portal 同处 `10.11.0.0/8`。
- 浏览器能路由到 `10.11.0.3:3015`，否则用 SSH 端口转发。
- 单管理员，无并发编辑冲突需求。
- 校验用的 `sing-box` 二进制由部署方通过 `SING_PORTAL_SING_BOX` 提供。

不做：

- 登录、会话、RBAC、TLS、公网入口。
- NixOS 模块、sops、systemd unit、nginx 配置（都在 `nix/infra`）。
- 订阅分组、按机器白名单、多租户。
- Clash 面板全功能（yacd 类）、自动选优/健康检查回滚。
- `ssh`/`wireguard`/`hysteria`(v1) 等少见协议的 URI 转换。
