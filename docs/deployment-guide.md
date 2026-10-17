# WaLiAPI 部署文档

> 适用版本：**v0.3.1**（版本号需在 `package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`、`src-tauri/Cargo.lock` 四处保持一致）
> 覆盖三种部署形态，并在末尾给出一次真实生产部署的完整记录（Docker + nginx-proxy-manager + 自动 HTTPS）。

---

## 1. 选型：先确定要哪种产物

同一套 Rust 代码编译出两种产物（`src-tauri/Cargo.toml:99-104`）：

| 形态 | 入口/产物 | Cargo feature | 界面 | 适用场景 |
|:---|:---|:---|:---|:---|
| 桌面端 | `waliapi`（`src-tauri/src/main.rs`） | 默认 `desktop-ui` | Tauri 2 窗口 | 个人本机使用，零运维 |
| Headless 服务端 | `waliapi-web`（`src-tauri/src/bin/waliapi-web.rs`） | `--no-default-features --features embed-web` | 内嵌 Web 管理面板 | 云服务器 / 团队共享 / 容器化 |

**关键点**：headless 构建**必须**加 `--no-default-features`。`desktop-ui` 会把托盘、文件对话框、自启、GTK/WebKit/DBus 链进二进制；关掉后 `embed-web` 通过 rust-embed 把 `web/dist` 内嵌进二进制，因此**不需要单独的静态资源目录**，一个二进制即可对外服务。

三种形态的能力差异只有"桌面专属交互"（文件选择框、系统托盘、浏览器唤起、开机自启）；网关转发、渠道调度、知识库、MCP、安全审计等核心能力完全一致。

---

## 2. 前置条件

### 2.1 桌面端

按 Tauri 2 的标准要求准备工具链：Node 20+/pnpm、Rust stable、平台原生依赖（Windows 用 MSVC + WebView2；macOS 用 Xcode Command Line Tools；Linux 需 `libwebkit2gtk-4.1-dev`、`libgtk-3-dev`、`librsvg2-dev`、`libayatana-appindicator3-dev`、`patchelf` 等）。上面这串 Linux 包名不是猜的——Dockerfile 的构建阶段装的就是同一套（`Dockerfile:28-39`），可当作用包清单参考。

```bash
pnpm install --frozen-lockfile
pnpm tauri dev            # 开发模式（等价 ./start.sh）
pnpm tauri build          # 出安装包（.msi / .dmg / .deb / .AppImage）
```

### 2.2 服务端（Docker 或裸机二进制）

- **Docker Engine 20.10+**（需要 BuildKit，多阶段构建用）
- 或裸机：Rust stable + Node 20+/pnpm
- **必须准备两个强随机令牌**（≥32 字符，且两者不同）：

```bash
openssl rand -hex 32      # → WALIAPI_ADMIN_TOKEN
openssl rand -hex 32      # → WALIAPI_MCP_TOKEN
```

- 资源建议：镜像约 932 MB，运行内存常驻约 100 MB 量级；**构建机的内存要求远高于运行机**（见 3.4 的 LTO 说明）。

---

## 3. 方式 A：Docker 部署（服务器首选）

### 3.1 快速启动（预构建镜像）

```bash
export WALIAPI_ADMIN_TOKEN="$(openssl rand -hex 32)"
export WALIAPI_MCP_TOKEN="$(openssl rand -hex 32)"

docker run -d --name waliapi \
  -p 127.0.0.1:8777:8777 \
  -v waliapi-data:/data \
  -e WALIAPI_ADMIN_TOKEN \
  -e WALIAPI_MCP_TOKEN \
  fuzhengwei/waliapi:latest

curl http://127.0.0.1:8777/health
```

`-p 127.0.0.1:8777:8777` 是刻意的：**默认只发布到宿主机回环地址**，公网访问必须经由反向代理终止 HTTPS（`docker-compose.yml:10-12` 有同样的注释）。

### 3.2 从源码构建

```bash
git clone https://github.com/fuzhengwei/WaLiAPI.git && cd WaLiAPI
export WALIAPI_ADMIN_TOKEN="$(openssl rand -hex 32)"
export WALIAPI_MCP_TOKEN="$(openssl rand -hex 32)"
docker compose up -d --build
curl http://127.0.0.1:8777/health
```

Compose 定义（`docker-compose.yml`）要点：

- 数据落在**命名卷** `waliapi-data` → 容器内 `/data`（不要换成宿主机 bind mount，见 3.5）
- `restart: unless-stopped` + `init: true`
- 内置 healthcheck：`curl -f http://127.0.0.1:8777/health`，30s 间隔、20s 启动宽限期
- `WALIAPI_SERVER_HOST=0.0.0.0`、`WALIAPI_DATA_DIR=/data`、`WALIAPI_TARGET_HOME=/data/managed-home`
- 两个 token 用 `${VAR:?}` 强校验，缺一个直接启动失败——这是有意设计，避免无鉴权裸奔

### 3.3 反向代理与 HTTPS

容器监听 `0.0.0.0:8777`，宿主机只映射到 `127.0.0.1:8777`，前面接 Caddy / Nginx / nginx-proxy-manager：

```caddyfile
api.example.com {
    reverse_proxy 127.0.0.1:8777
}
```

Caddy 会自动申请并续期证书（示例见 `deploy/caddy/Caddyfile.example`）。

**反向代理侧有两个必改项**（否则流式会被破坏，与"能不能用"直接相关）：

```nginx
proxy_buffering off;          # 必须关：否则 SSE 会被缓冲成一次性吐出，流式退化为非流式
proxy_read_timeout 3600s;     # 建议放大：Nginx 默认 60s 会掐断长文本生成
client_max_body_size 100m;    # 按需：知识库上传大文档时需要
```

补充两点常见误解：

- **WebSockets 开关与 WaLiAPI 无关**。本项目前后端**没有任何 WebSocket 代码**，实时通道走的是 SSE（`/admin/api/events` 事件桥 + `/v1/*` 流式响应），普通 HTTP chunked 即可承载。反代里开不开 WebSocket 升级都不影响功能。
- 认证头**不得**在反代层移除或改写（`Authorization` / Cookie 都要透传）。

### 3.4 国内构建加速（重要，能省掉数小时）

`Dockerfile` 已把 Rust 工具链源默认指到 `rsproxy.cn`，并暴露了 apt 与 crates 的构建参数：

| 构建参数 | 默认值 | 国内建议值 |
|:---|:---|:---|
| `DEBIAN_MIRROR` | `http://deb.debian.org/debian` | `http://mirrors.aliyun.com/debian` |
| `DEBIAN_SECURITY_MIRROR` | `http://deb.debian.org/debian-security` | `http://mirrors.aliyun.com/debian-security` |
| `RUSTUP_MIRROR` | `https://rsproxy.cn` | 保持默认 |
| `CARGO_REGISTRY_MIRROR` | `sparse+https://rsproxy.cn/index/` | 保持默认 |
| `NPM_REGISTRY` | `https://registry.npmjs.org/` | `https://registry.npmmirror.com`（需先按下方说明给 Dockerfile 打补丁） |

三个实测踩过的坑：

1. **apt 源必须是纯 HTTP**。构建/运行镜像用的是 `debian:bookworm-slim`，**该镜像不含 `ca-certificates`**；把 apt 源写成 `https://` 会直接报 `No system certificates available`，紧接着变成 `Couldn't find any package by regex 'libwebkit2gtk-4.1-dev'` 这种极具误导性的报错。别从这里开始排查包不存在。
2. **npm 走哪个源由环境变量决定，但上游 Dockerfile 没有开这个口子**。上游 `Dockerfile` 只用 `pnpm install --frozen-lockfile`，没有 `ARG NPM_REGISTRY`。若构建机到 npmjs 很慢，需要给 Dockerfile 加一个参数（本次部署即如此，见第 6 节）。
3. **pdfium 动态库默认从 GitHub Releases 拉**（`scripts/fetch-pdfium.sh`）。国内直连 GitHub Releases 常常只有几十 KB/s。该脚本是幂等的（文件已存在且校验通过就跳过），所以可以**先把 `pdfium-linux-x64.tgz` 解出 `libpdfium.so` 放到 `src-tauri/resources/pdfium/`**，或在能跑通的环境里先拉一次再打包传过去。脚本会校验大小 >1MB 且 ELF 魔数 `7f454c46`，放错文件会直接失败而不是静默通过。

**LTO 与内存**：release profile 是 `lto = "fat"` + `codegen-units = 1`（`src-tauri/Cargo.toml:106-113`）。这个组合让最终 codegen 基本单线程跑，**内存峰值远高于并行编译**。2 核 8G 这类小机器建议临时加 swap 再构建：

```bash
fallocate -l 4G /swapfile && chmod 600 /swapfile && mkswap /swapfile && swapon /swapfile
# 构建完成后：swapoff /swapfile && rm /swapfile
```

> 注意 `profile.release` 里**不要**加 `panic = "abort"`——代码里知识库解析等护栏依赖 `catch_unwind`，加了会让这些保护失效（Cargo.toml 中有注释说明）。

### 3.5 关于 `/data` 的权限陷阱

容器以非 root 用户 `waliapi`（uid **10001**，`Dockerfile:90`）运行。如果为了"方便"把 `/data` 换成宿主机目录 bind mount，**宿主机目录属主不是 10001 时容器起不来**，报错是 `failed to connect to database: unable to open database file`——看起来像数据库损坏，实则是目录不可写。

- 首选：用命名卷（Compose 默认行为），卷会继承镜像里 `/data` 的属主；
- 确要用 bind mount：先 `mkdir -p /your/dir && chown -R 10001:10001 /your/dir`。

---

## 4. 方式 B：systemd 裸机部署

适合不想引入 Docker 的场景。先编二进制（**在 `src-tauri/` 目录下跑 cargo**）：

```bash
pnpm install --frozen-lockfile
pnpm --filter waliapi-web build
cargo build --release --manifest-path src-tauri/Cargo.toml \
  --bin waliapi-web --no-default-features --features embed-web
```

然后按仓库内的示例安装：

```bash
sudo install -Dm600 deploy/systemd/waliapi.env.example /etc/waliapi/waliapi.env
sudo chown root:root /etc/waliapi/waliapi.env
sudoedit /etc/waliapi/waliapi.env        # 填 WALIAPI_ADMIN_TOKEN / WALIAPI_MCP_TOKEN

sudo install -Dm644 deploy/systemd/waliapi.service /etc/systemd/system/waliapi.service
sudo systemctl daemon-reload
sudo systemctl enable --now waliapi
sudo systemctl status waliapi
```

`deploy/systemd/waliapi.service` 的要点：

- `User=waliapi` / `Group=waliapi`，`WorkingDirectory=/opt/waliapi`
- `EnvironmentFile=/etc/waliapi/waliapi.env`
- `StateDirectory=waliapi` → 由 systemd 创建并授权 `/var/lib/waliapi`（**数据目录默认落这里**；若改用其他路径，必须同步放开 unit 的可写范围，否则被 `ProtectSystem=strict` 拦住）
- 加固项：`NoNewPrivileges=true`、`PrivateTmp=true`、`ProtectSystem=strict`、`ProtectHome=true`
- `Restart=on-failure` / `RestartSec=5s`

---

## 5. 环境变量与数据目录

### 5.1 全部环境变量

| 变量 | 说明 | 默认 |
|:---|:---|:---|
| `WALIAPI_SERVER_HOST` | 服务监听地址 | `127.0.0.1`（桌面）/ `0.0.0.0`（Docker） |
| `WALIAPI_SERVER_PORT` | 服务监听端口 | `8777` |
| `WALIAPI_DATA_DIR` | 数据目录（SQLite + 知识库索引 + 日志） | Docker `/data`；systemd `/var/lib/waliapi` |
| `WALIAPI_TARGET_HOME` | 托管 HOME（OAuth 登录态等落在这里） | Docker `/data/managed-home` |
| `WALIAPI_ADMIN_TOKEN` | **KB/Wiki REST**（`/api/kb`、`/api/wiki`）Bearer 令牌，≥32 字符 | 未设 → 这些端点关闭，一律 401 |
| `WALIAPI_MCP_TOKEN` | **MCP 端点**（`/mcp`）Bearer 令牌，≥32 字符，**须与 ADMIN 不同** | 未设 → 端点关闭，一律 401 |
| `WALIAPI_PUBLIC_URL` | 对外公网地址，生成客户端接入配置时使用 | 空 |
| `WALIAPI_PDFIUM_PATH` | pdfium 动态库路径（OCR 用） | `Dockerfile` 设为 `/usr/local/lib/waliapi/pdfium` |
| `WALIAPI_BIND` / `WALIAPI_PORT` | **Compose 专用**：宿主机侧发布地址/端口 | `127.0.0.1` / `8777` |
| `WALIAPI_DATA_DIR`（回退链） | 数据目录解析优先级：**显式参数 > `WALIAPI_DATA_DIR` > `XDG_DATA_HOME` > 平台应用数据目录** | — |
| `RUST_LOG` | 日志级别过滤 | Docker 内 `info`（stdout + 按天滚动文件双写） |

### 5.2 数据目录内容

以容器内 `/data` 为例（实测）：

```
/data/
├── waliapi.db          # SQLite 主库（渠道、密钥、日志、知识库元数据…）
├── logs/               # 按天滚动日志（waliapi.log.YYYY-MM-DD，保留 7 个）
├── settings.json       # headless 模式的设置存储
├── managed-home/       # 托管 HOME（OAuth 登录态）
├── INITIAL_PASSWORD    # 首次启动生成的管理员临时密码；首登成功后自动删除
└── （知识库向量索引等）
```

**单实例约束**：SQLite 不支持多个写实例共享同一数据目录，**必须保持单实例**。滚动升级时先停旧实例再起新实例。

---

## 6. 本次真实部署记录（Docker + NPM + HTTPS）

> 环境：Ubuntu 22.04、2 核 8G / 30G 盘、Docker；仓库落在 `/DJG/app/WaLiAPI`；
> 反代由宿主机已有的 **nginx-proxy-manager 2.15.1** 承担（无宿主机 nginx）；
> 对外域名 `waliapi.xj0.ltd`，管理面板与 LLM 网关**同域**。

### 6.1 部署要点回顾

1. **源码传输必须保持 LF 行尾**。源码从 Windows 打包传到 Linux 时，若本机 `core.autocrlf=true`，工作区里是 CRLF，会被原样带过去。CRLF 会让 `bash scripts/fetch-pdfium.sh` 这类脚本在容器内直接失败（`\r` 被当成命令的一部分）。校验方式：`git diff --ignore-cr-at-eol` 应为空。修正：`git -c core.autocrlf=false checkout -- .`。
   另外跨用户传输后 git 可能报 `dubious ownership`，`chown -R root:root <repo>` 即可。
2. **给 Dockerfile 加 npm 源参数**（上游没有）。仓库内该文件保持上游原样，服务器副本上打了这个补丁——补丁本身是向后兼容的，默认值仍是官方源：

   ```diff
   +ARG NPM_REGISTRY=https://registry.npmjs.org/
    ...
   +ARG NPM_REGISTRY
    ENV ... \
        RUSTUP_UPDATE_ROOT=${RUSTUP_MIRROR}/rustup \
   +    NPM_CONFIG_REGISTRY=${NPM_REGISTRY} \
   +    COREPACK_NPM_REGISTRY=${NPM_REGISTRY} \
        PATH=/usr/local/cargo/bin:$PATH
   ```

3. **构建命令**（实测可用）：

   ```bash
   docker build \
     --build-arg DEBIAN_MIRROR=http://mirrors.aliyun.com/debian \
     --build-arg DEBIAN_SECURITY_MIRROR=http://mirrors.aliyun.com/debian-security \
     --build-arg NPM_REGISTRY=https://registry.npmmirror.com \
     -t waliapi:0.3.1 -t waliapi:latest .
   ```

   该机实测出网吞吐（可作为选源依据）：`deb.debian.org` 索引 ~23 KB/s、npmjs ~388 KB/s、GitHub Releases 直连 ~14 KB/s；`mirrors.aliyun.com/debian` ~695 KB/s、`registry.npmmirror.com` ~7.4 MB/s、`gh-proxy.com` ~3.1 MB/s。`mirrors.tencentyun.com` 该机不可达，`mirrors.tencent.com` 会 302 跳 https（于是又撞上 ca-certificates 那个坑）。

4. **让容器接入 NPM 所在的 docker 网络**。新增 `docker-compose.override.yml`（**不改动上游跟踪的 `docker-compose.yml`**）：

   ```yaml
   services:
     waliapi:
       networks:
         - default
         - proxy-net

   networks:
     proxy-net:
       external: true
   ```

   这样 NPM 就能用容器名 `waliapi:8777` 直连，无需经过宿主机端口。

5. **NPM 侧新增 Proxy Host**：域名 `waliapi.xj0.ltd` → `http://waliapi:8777`，Advanced 里填 3.3 的三条 `proxy_buffering off; proxy_read_timeout 3600s; client_max_body_size 100m;`，并申请 HTTPS 证书。

6. **DNS**：DNSPod 加 A 记录指向服务器公网 IP。

### 6.2 联调顺序（建议照这个顺序排障）

域名解析未生效时，不要干等，逐层往上验：

```bash
# ① 容器自身健康
ssh <server> 'docker compose ps'                       # Up (healthy)
ssh <server> 'curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:8777/health'   # 200

# ② 只验反代路由（绕过 DNS，用 Host 头模拟）
ssh <server> 'curl -s -o /dev/null -w "%{http_code}\n" -H "Host: waliapi.xj0.ltd" http://127.0.0.1/'

# ③ 验完整外部链路（绕过 DNS，用 --resolve 指定 IP）
curl -s -o /dev/null -w "%{http_code} %{time_total}s\n" \
  --resolve waliapi.xj0.ltd:443:<公网IP> https://waliapi.xj0.ltd/login

# ④ 验证 DNS 传播（多解析器交叉确认）
for r in 8.8.8.8 1.1.1.1 114.114.114.114 223.5.5.5; do
  echo -n "$r: "; dig +short @$r waliapi.xj0.ltd
done
```

第 ④ 步若部分解析器仍返回空，多半是**负数缓存**：DNSPod 默认 SOA minimum 为 180 秒，等几分钟即可，不需要改配置。

**看日志定位"请求到底有没有打到后端"**：NPM 的访问日志在容器内 `/data/logs/proxy-host-<id>_access.log`，格式里带 `Sent-to waliapi` 和上游状态码。例如登录失败时能看到：

```
POST https waliapi.xj0.ltd "/admin/api/auth/login" [Client ...] [Length 36] "..." "https://waliapi.xj0.ltd/login"
```

`Length 36` 恰好是 `{"error":"用户名或密码错误"}` 的字节数——**这类"看响应体长度就能对上错误语义"的判据，比反复猜密码高效得多**。

### 6.3 最终状态（实测）

```
NAME                IMAGE            STATUS                    PORTS
waliapi-waliapi-1   waliapi:latest   Up (healthy)              127.0.0.1:8777->8777/tcp

镜像：waliapi:0.3.1 / waliapi:latest  932MB
数据卷：waliapi_waliapi-data
```

NPM 侧 proxy host 实测配置：

| 项 | 值 |
|:---|:---|
| domain | `waliapi.xj0.ltd` |
| forward | `http` → `waliapi:8777` |
| advanced_config | `proxy_buffering off;` `proxy_read_timeout 3600s;` `client_max_body_size 100m;` |
| 证书 | 已挂载（自动申请） |
| `ssl_forced` | 建议置 **true**（否则明文 HTTP 仍可用，登录口令走 POST body 存在降级风险） |
| `block_exploits` | 建议置 **true**（面板公网可达，实测有扫描器在探测 `/graphql`、`/.env`、`/config.json`） |
| `allow_websocket_upgrade` | 开或不开都不影响功能（本项目无 WebSocket） |

> 建议进一步把 `ssl_forced` 与 `block_exploits` 打开：都是一个开关、可逆。

---

## 7. 首次登录与认证体系

### 7.1 三个互不通用的凭证域

| 用途 | 凭证 | 说明 |
|:---|:---|:---|
| 数据面 `/v1/*` | `sk-waliapi-*` 网关密钥 | 后台创建，供下游客户端调用网关 |
| Web 管理面 + KB/Wiki REST | 管理面板**用户名/密码会话**；KB/Wiki REST 用 `WALIAPI_ADMIN_TOKEN` | 两者是不同入口，别混用 |
| MCP 端点 `/mcp` | `WALIAPI_MCP_TOKEN` | 与 ADMIN token 权限隔离，必须不同值 |

宽松 CORS 只作用于数据面 `/v1/*`（跨域调用网关是设计用法）；管理面、KB/Wiki、MCP **不下发 CORS 允许头**，网页跨域读不到响应。管理面另有 CSRF 防护：**变更类请求（POST/PUT/PATCH/DELETE）必须携带 `X-Requested-With: XMLHttpRequest`**，否则 403。用 curl 手测管理接口时要记得带上这个头。

### 7.2 获取初始管理员密码

首次启动且 `admin_users` 表为空时，自动创建 `admin` 账号并生成 **16 位随机密码**（`src-tauri/server/admin_auth.rs:282-310`），同时：

- 打印到**容器 stdout**（`docker logs` 可见）
- 写入 **`<数据目录>/INITIAL_PASSWORD`**，内容两行：`username: admin` / `password: <16 位>`
- 账号置 `must_change_password = 1`，登录后前端强制跳转改密页

```bash
# 看日志（首次启动的输出很直观）
docker logs waliapi 2>&1 | grep -A4 '初始账号'
# 或直接读文件
docker exec waliapi cat /data/INITIAL_PASSWORD
```

**首次登录成功即删除该文件**（`server/admin_routes.rs:230-237`），因为持有人已知密码，明文落盘没必要。所以这个文件**读一次就没了**，请当场记下或直接改密。文件消失后仍可用密码正常登录——密码哈希在库里。

### 7.3 改密与登录保护

- 改密接口 `POST /admin/api/auth/change-password`，请求体 `{old_password, new_password}`，**新密码硬性要求 ≥8 位**（`server/admin_routes.rs:293`），短于此直接 `400 新密码长度至少 8 位`。
- 改密成功后当前会话续期、**其他所有旧会话全部吊销**（`server/router.rs` 的 `admin_login_flow_hardening` 测试覆盖了这条）。
- 登录失败限速：按用户名 + 全局两级指数退避，**前 5 次免罚**，之后 2^n 秒、**封顶 1 小时**，命中返回 `429` 并带 `Retry-After`（`server/admin_auth.rs:105-177`）。
- 会话 Cookie 名 `waliapi_admin_token`，`HttpOnly` + `SameSite=Lax`，有效期 **7 天**；同时接口也接受 `Authorization: Bearer <会话 token>`。

### 7.4 忘记密码怎么恢复

`waliapi-web` 二进制**没有** `reset-password` 之类的子命令（CLI 只支持 `[start]` 与 `repair-stream-logs`，见 `src-tauri/src/bin/waliapi-web.rs`）。恢复路径是利用"表为空则重建 admin"的启动逻辑：

```bash
# 1) 停实例（SQLite 单写者，务必先停）
docker compose stop waliapi

# 2) 删掉 admin 账号行（假设数据在命名卷里）
docker run --rm -v waliapi_waliapi-data:/data alpine \
  sh -c "apk add -q sqlite && sqlite3 /data/waliapi.db 'DELETE FROM admin_users;'"

# 3) 重启 → 重新生成 INITIAL_PASSWORD
docker compose start waliapi
docker exec waliapi cat /data/INITIAL_PASSWORD
```

> 这等价于一次管理员账号重置，会同时清掉 `must_change_password` 状态和其他会话。请只在确实进不去时使用，并确保操作期间没有其他实例在写库。

---

## 8. 备份、升级与回滚

### 8.1 备份

数据库是**默认 `journal_mode=delete`（回滚日志）**的 SQLite，写事务提交需要 EXCLUSIVE 锁。因此：

- **不要在网关在线写入时直接 `cp waliapi.db`**，可能拿到撕裂快照；
- 首选停机窗口备份，或用 SQLite 一致性方式：

```bash
docker compose stop waliapi
docker run --rm -v waliapi_waliapi-data:/data -v "$PWD:/backup" alpine \
  sh -c "apk add -q sqlite && sqlite3 /data/waliapi.db \".backup '/backup/waliapi-$(date +%F).db'\""
docker compose start waliapi
```

- 需要连知识库索引一起备份时，直接归档整个 `/data`。
- 提醒：`/data` 里的渠道密钥与网关 Key 是**明文**存储的（见第 10 节），备份文件属于敏感数据，注意权限与传输加密。

### 8.2 升级

```bash
git pull                          # 或换到目标 tag
git checkout v0.3.2
export WALIAPI_ADMIN_TOKEN=...    # Compose 的 ${VAR:?} 校验要求这两个变量在场
export WALIAPI_MCP_TOKEN=...
docker compose build              # 别忘了重新带上镜像源构建参数
docker compose up -d
docker compose ps                 # 期望 Up (healthy)
```

数据库迁移**启动时自动执行**（`sqlx::migrate!`），且迁移前会自动备份数据库并保留最近若干份，因此升级前不必手工导库；但仍建议在升级前留一份可回滚副本。

### 8.3 回滚

镜像按版本打了 tag（`waliapi:0.3.1`），回滚只需切回旧 tag 重启。**注意迁移是前向的**：若新版本已执行过不兼容的 schema 变更，仅切回旧镜像可能不够，需一并恢复升级前的数据库备份。所以"升级前留备份"不是仪式。

---

## 9. 日常运维手册

```bash
# 状态与健康
docker compose ps
curl -s http://127.0.0.1:8777/health

# 日志（stdout）
docker logs -f --tail 200 waliapi
# 日志文件（按天滚动，保留 7 个）
ls /data/logs/            # 容器内；或 docker exec waliapi ls -l /data/logs

# 进入容器
docker exec -it waliapi sh

# 数据卷位置
docker volume inspect waliapi_waliapi-data

# 资源占用
docker stats waliapi --no-stream
```

### 9.1 流式日志修复工具（离线维护命令）

历史版本存在"被误记为 499 / client_cancelled 且 token 用量为 0"的流式日志。`waliapi-web` 内置一次性修复命令：

```bash
waliapi-web repair-stream-logs [--data-dir <目录>] [--apply] [--limit <行>]
# 默认 dry-run 只报告不改库；加 --apply 才写库
# --status-only  只改判状态（499→200），秒级完成
# --usage-only   只补 token 用量与响应内容（需跑 BPE，大库分钟级）
```

**执行前必须先停掉正在使用同一数据目录的实例**。原因在命令自身的帮助里写得很直白：该命令会连续发起数百个写事务，与在线服务争 EXCLUSIVE 锁，两侧都会 `SQLITE_BUSY` 停顿；而且首次执行会触发 schema 迁移，迁移前的 `VACUUM INTO` 备份需要整库一致性快照，在 GB 级库上尤其不适合与在线写入并行。

### 9.2 常见故障对照

| 现象 | 可能原因 | 处理 |
|:---|:---|:---|
| 容器起不来，日志 `unable to open database file` | `/data` 属主不是 uid 10001 | 用命名卷，或 `chown -R 10001:10001` |
| 启动即失败，提示缺少 token | `WALIAPI_ADMIN_TOKEN` / `WALIAPI_MCP_TOKEN` 未设 | Compose 用 `${VAR:?}` 强校验，必须注入 |
| 登录页报"用户名或密码错误" | 密码输错（注意大小写与空格） | 见 7.2 取初始密码；连续失败会触发 429 限速，等 `Retry-After` 秒 |
| 连续登录被 `429` | 触发指数退避 | 按 `Retry-After` 秒数等待；前 5 次失败免罚 |
| 管理接口用 curl 手测返回 403 `CSRF 校验失败` | 变更类请求缺自定义头 | 加 `-H 'X-Requested-With: XMLHttpRequest'` |
| `/api/kb`、`/api/wiki` 一律 401 | `WALIAPI_ADMIN_TOKEN` 未配置 | 该情形下端点是"关闭"而非"开放"，属于 fail-closed |
| `/mcp` 一律 401 | `WALIAPI_MCP_TOKEN` 未配置或用了 ADMIN token | 两个凭证域互不通用，必须用 MCP token |
| 流式响应变成一次性吐出 | 反代开了 `proxy_buffering` | 反代加 `proxy_buffering off;` |
| 长文本生成中途断流 | 反代读超时过短 | 反代 `proxy_read_timeout 3600s;` |
| 构建报 `Couldn't find any package by regex 'libwebkit2gtk-4.1-0'` | apt 源写成了 https，而 slim 镜像无 ca-certificates | apt 源改纯 HTTP（见 3.4） |
| 构建 OOM / 被 OOM Killer 杀 | `lto=fat` + `codegen-units=1` 的最终 codegen 内存峰值 | 临时加 swap（见 3.4） |
| `bash scripts/fetch-pdfium.sh` 报奇怪的命令错误 | 源码被检出成 CRLF 行尾 | 归一到 LF（见 6.1） |
| Windows 上 `git status` 显示大量文件被改动 | `core.autocrlf` 导致的纯行尾差异 | `git diff --ignore-cr-at-eol` 确认为空即可 |

### 9.3 磁盘回收

镜像与构建缓存比镜像本身大得多，长期部署的机器建议定期清理：

```bash
docker builder prune -f        # 回收 BuildKit 缓存（本机实测可回收约 6.4GB）
docker image prune -f          # 清理悬空镜像
docker system df               # 先看占用分布再决定
```

本次部署后宿主机 30G 盘用掉 70%（可用 9.0G），其中构建缓存是最大头，回收前请确认没有其他项目正在复用缓存。

---

## 10. 安全声明：公网部署前必须知道的事

### 10.1 明文密钥存储（上游知情声明）

上游渠道密钥与网关 API Key **以明文存储在本地 SQLite 中**，本项目不提供静态加密——**数据目录的文件系统权限就是安全边界**。渠道导出文件包含明文密钥（界面有明示警告），请仅在受控环境操作；网关密钥在管理界面只显示掩码，仅在显式复制动作时取回全量。

这意味着：`/data`（含其备份、快照、迁移到别的机器上的副本）失守 = 上游供应商密钥与网关 Key 全部失守。请把 `/data` 当凭证库对待。

### 10.2 公网暴露检查清单

本项目 README 自我定位为**本地 / 内网优先**的网关。确需公网暴露时，逐项确认：

- [ ] 容器端口只发布到 `127.0.0.1`，由反代终止 TLS，**不要** `-p 8777:8777` 直接暴公网
- [ ] `ssl_forced` 打开，避免明文 HTTP 降级（登录口令是 POST body）
- [ ] `WALIAPI_ADMIN_TOKEN` / `WALIAPI_MCP_TOKEN` 都是 `openssl rand -hex 32` 级别，且两者不同
- [ ] 管理面板密码已从初始临时密码改为强密码（≥8 位是下限而非目标）
- [ ] 反代打开常见攻击拦截（NPM 的 `block_exploits` 等）；面板域名必然会被扫描器光顾
- [ ] 反向代理**不移除、不改写**认证头（`Authorization` / Cookie）
- [ ] 明确知识库与 Wiki 的内容敏感度——这部分是"AI Agent 可直接检索的私有知识"
- [ ] 安全扫描按"尽力而为的 DLP"理解：它是启发式规则匹配，**不是**完整内容安全方案

### 10.3 内置安全能力（开箱可用）

- 风险扫描引擎：敏感信息、路径、Unicode 隐写等 25+ 内置规则，策略支持只审计 / 警告 / 脱敏 / 阻断
- 响应侧扫描覆盖非流式、流式与原生 Anthropic 路径；扫描异常不阻断转发，但**扫描预算超限按 fail-closed 拒绝请求**
- 管理面：登录限速（指数退避）、会话 Cookie `HttpOnly`、改密吊销全部旧会话、CSRF 防护、时序防用户名枚举
- 服务端点 fail-closed：未配置 token 时一律 401，绝不无鉴权放行

---

## 11. 约束与已知坑速查

| 约束 | 说明 |
|:---|:---|
| **单实例** | SQLite 不支持多实例写同一数据目录。滚动升级、维护命令都必须先停旧实例 |
| **headless 必须 `--no-default-features`** | 漏了会把 GTK/WebKit/DBus 链进二进制 |
| **release profile 勿改** | `lto="fat"` + `codegen-units=1` 用于剔除桌面端代码；`panic="abort"` **绝对不要加**（会让 `catch_unwind` 护栏失效） |
| **迁移只增不改** | 新增迁移按 `NNN_name.sql` 顺延编号，不要改动已发布的迁移文件 |
| **版本号四处同步** | `package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`、`src-tauri/Cargo.lock` |
| **源码行尾必须 LF** | 否则容器内 shell 脚本会失败 |
| **slim 镜像无 ca-certificates** | apt 源必须纯 HTTP；需要 https 时先装 `ca-certificates` |
| **pdfium 需另行获取** | 库文件不入库；构建阶段由 `scripts/fetch-pdfium.sh` 下载，国内建议预放置 |
| **`/data` 属主 uid 10001** | bind mount 场景下必须 `chown` |

---

## 12. 参考

| 文件 | 内容 |
|:---|:---|
| `docker-compose.yml` | Compose 定义、端口绑定策略、healthcheck |
| `Dockerfile` | 多阶段构建、构建参数、非 root 运行 |
| `deploy/systemd/waliapi.service` | systemd unit（含沙箱加固） |
| `deploy/systemd/waliapi.env.example` | 环境文件模板 |
| `deploy/caddy/Caddyfile.example` | Caddy 反代示例 |
| `scripts/fetch-pdfium.sh` | OCR 渲染依赖的获取与校验逻辑 |
| `docs/architecture-overview.md` | 项目架构解析（模块、链路、数据模型） |
| `README.md` | 产品功能与版本历史 |
| `AGENTS.md` | 面向 AI 编码代理的开发约定（注意部分内容滞后于代码，见架构文档第 24 节） |