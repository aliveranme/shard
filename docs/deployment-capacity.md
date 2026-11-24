# Shard 部署配置、资源限制与扩容空间

选择部署时先确认需要扩大的是单次 Git push、整个仓库、单个 LFS 对象，还是搜索、合并和后台作业的吞吐。Workers 套餐决定计算和服务额度，域名套餐决定经 Cloudflare 入口的单请求体积，Git 存储后端决定 pack 和仓库容量。下文使用 Cloudflare 原文的 MB、GB 与 MiB、GiB，两个单位体系分别按十进制和二进制计量。[Workers 限制](https://developers.cloudflare.com/workers/platform/limits/) · [Artifacts 限制](https://developers.cloudflare.com/artifacts/platform/limits/)

首次部署可先看 [Cloudflare Dashboard 中文配置手册](configuration.zh-CN.md)，其中有一键部署字段、Git 绑定、构建令牌和各环境的逐项操作。

## 三种部署入口

新建小型实例可用 Free 按钮。已有 GitHub 或 GitLab 仓库、希望提交后自动发布时，用 Dashboard 连接 Git。希望部署固定版本并跳过 Rust 构建时，用 Release 预构建包。需要 Cloudflare 托管的大 Git push 时选择 `r2`。已有带持久盘的 Git 主机，可用 `native` 直连，让标准 Git 直接传大 pack。

### Cloudflare Dashboard 从 Git 部署

1. 把 Shard fork 到自己的 GitHub 或 GitLab 账号。部署 Free 时，保留顶层的 `GIT_REPOS`、`free-git-v1` 迁移与 `DB` 绑定。按需修改顶层 `name` 和 D1 的 `database_name`。[配置样例](../wrangler.jsonc)
2. 在 [Cloudflare Dashboard](https://dash.cloudflare.com/?to=/:account/workers-and-pages) 打开 **Workers & Pages → Create application**，选择导入已有 Git 仓库，授权并选中 fork。已有 Worker 可在 **Settings → Builds** 连接仓库。[Git 接入说明](https://developers.cloudflare.com/workers/ci-cd/builds/git-integration/)
3. 选择发布分支（通常为 `main`）。构建根目录用仓库根目录，**Build command** 填 `npm run build`，**Deploy command** 填 `npm run deploy`。为构建选择目标账号下具有 **Workers Scripts Edit**、**D1 Edit** 的 API 令牌。发布 zone 路由时还需 **Workers Routes Edit**。部署脚本先应用 D1 迁移，再上传 Worker。部署完成后打开 `workers.dev` 地址。[Workers Builds 设置](https://developers.cloudflare.com/workers/ci-cd/builds/configuration/)
4. 运行时密钥从 Worker 的 **Settings → Variables and Secrets** 添加为 **Secret**。在 Wrangler 的对应 `vars` 中管理文本配置。若改由 Dashboard 管理某个文本值，移除文件中同名的 `vars` 项，让 `keep_vars` 保留线上值。[变量和密钥](../README.md#worker-variables-and-secrets)

**在 Dashboard 配置 Paid 分片。** 开通 Workers Paid 和 Containers。创建目标环境的私有 R2 桶、桶级 **Object Read & Write** 凭据及下文的暂存生命周期规则。选择 `managed` 时还需启用 Artifacts 并配置命名空间。先创建同名 Worker，在第一次 Shard 构建前添加运行时 Secret，再连接 Git 和发布分支。对应 `env.<环境>.name` 必须与 Dashboard 的 Worker 名一致。带 R2 的构建令牌还需 **Workers R2 Storage Edit**。[R2 凭据](https://developers.cloudflare.com/r2/api/tokens/) · [容器构建](https://developers.cloudflare.com/containers/guides/deploy/#deploy-with-workers-builds)

| 范围 | 所选环境的 `vars` | 在对应 Worker 添加的 Secret |
| --- | --- | --- |
| 两种 Paid 分片环境 | 默认 `SHARD_LIMIT_PROFILE=paid`、`SHARD_LFS_UPLOAD_MODE=r2-multipart`；在 `vars` 显式设置上传模式可覆盖默认值。`SHARD_LFS_R2_BUCKET` 和 `SHARD_LFS_R2_ENDPOINT` 指向 `LFS_OBJECTS` 对应的桶和 S3 endpoint | `SHARD_LFS_R2_ACCESS_KEY_ID`、`SHARD_LFS_R2_SECRET_ACCESS_KEY` |
| `r2` | 另设 `SHARD_GIT_R2_BUCKET` 和 `SHARD_GIT_R2_ENDPOINT`，对应独立的 `GIT_PACKS` 桶 | `SHARD_GIT_R2_ACCESS_KEY_ID`、`SHARD_GIT_R2_SECRET_ACCESS_KEY` |
| `managed` | 另设 `SHARD_GIT_ARTIFACTS_ORIGINS=https://<ACCOUNT_ID>.artifacts.cloudflare.net`，使用该账号的 Artifacts HTTPS 源站 | 使用本表的 LFS 凭据 |

在 R2 桶详情查看 **S3 API** 地址，将其 origin 填入 endpoint，例如 `https://<ACCOUNT_ID>.r2.cloudflarestorage.com`，桶名单独填写。**Build command** 仍用 `npm run build`。**Deploy command** 按环境用 `npm run deploy -- --env r2` 或 `npm run deploy -- --env managed`，读取已配置的变量和 Secret。Workers Builds 可从版本库的 Dockerfile 构建镜像。本地的 `deploy:paid` 会从 `--secrets-file` 读取凭据并推导同样的分片配置。需要 `managed-staging → managed` 的门禁时，用独立资源按托管 CI/CD 步骤配置两套环境。[容器发布](https://developers.cloudflare.com/containers/guides/deploy/) · [托管 CI/CD](../README.md#deploy-with-managed-cloudflare-services)

### 从 Release 包直接发布

1. 在 GitHub Releases 下载 `shard-v<版本>-worker.tar.gz` 和同名 `.sha256`。以下以 `v0.5.3` 为例。校验通过后解包，进入解出的目录。Windows PowerShell 可用 `Get-FileHash -Algorithm SHA256` 计算归档摘要，与 `.sha256` 文件中的值比对。[公开发布包](https://github.com/aliveranme/shard/releases/tag/v0.5.3)

   ```sh
   sha256sum --check shard-v0.5.3-worker.tar.gz.sha256
   tar -xzf shard-v0.5.3-worker.tar.gz
   cd shard-v0.5.3-worker
   ```

2. 按包内 `.node-version` 安装 Node，执行 `npm ci`。修改包内 `wrangler.json` 的目标 Worker 和资源名称。升级时填入原 D1 UUID、Git 绑定、R2 桶和路由。`managed` 与 `r2` 还需运行 Docker 或兼容引擎以构建容器镜像。包内已有 Worker Wasm 和静态资源，由 Wrangler 直接上传。Dashboard 的 **Upload your static files** 入口用于静态站点。[发布包内容](../README.md#project-releases)
3. 在包目录执行 `npx --no-install wrangler login`，用 `npx --no-install wrangler whoami` 确认账号。选择下表的一组命令。Paid 命令需在进程环境设置 `CLOUDFLARE_ACCOUNT_ID`，密钥文件按下方各环境的要求填写。

   | 目标 | 本地预检 | 发布 |
   | --- | --- | --- |
   | Free | `npm run deploy -- --dry-run` | `npm run deploy` |
   | 托管 Paid（Artifacts） | `npm run deploy:paid -- --dry-run` | `npm run deploy:paid -- --secrets-file .env.deploy.paid` |
   | 托管 Paid（R2 Git） | `npm run deploy:paid -- --env r2 --dry-run` | `npm run deploy:paid -- --env r2 --secrets-file .env.deploy.r2` |
   | Paid 原生 Git | `npm run deploy:paid -- --env native --dry-run` | `npm run deploy:paid -- --env native --secrets-file .env.deploy.paid` |

   旧 Artifacts 实例用 `npm run deploy -- --env artifacts`。预检检查打包和绑定解析。迁移、镜像发布和实际服务访问由正式部署执行。[旧实例升级](#旧-artifacts) · [Wrangler 部署](https://developers.cloudflare.com/workers/wrangler/commands/workers/#deploy)

### Free 一键部署按钮

1. 点击 [Deploy to Cloudflare Free](https://deploy.workers.cloudflare.com/?url=https%3A%2F%2Fgithub.com%2Faliveranme%2Fshard)，登录并选择 Cloudflare 账号。在「设置您的应用程序」的「Git 帐户」中选择 GitHub 或 GitLab，勾选「创建专用 Git 存储库」。[按钮流程](https://developers.cloudflare.com/workers/platform/deploy-buttons/)
2. 确认项目名称和 `DB` 数据库。根目录使用仓库根目录，保留 `npm run build` 和 `npm run deploy`。「高级设置」中的构建 API 令牌需要目标账号的 **Workers Scripts Edit** 和 **D1 Edit**。点击「部署」，完成后打开 `workers.dev` 地址。[构建设置](https://developers.cloudflare.com/workers/ci-cd/builds/configuration/)
3. Cloudflare 将代码复制到所选 Git 帐户并绑定新仓库。后续推送到生产分支会重新构建，迁移原 `DB`，更新同一个 Worker。合并 Shard 新版本时保留 Cloudflare 写入的 Worker 名和 D1 UUID。在 Worker 的「部署」页可查看构建记录和版本对应的 Git 提交。
4. 注册测试账号，创建私库和写入令牌，用 Git HTTPS 推送一个提交并克隆校验。需要 LFS 时，创建私有桶，在部署仓库的顶层 Wrangler 配置添加 `LFS_OBJECTS`，再推送至生产分支。需要域名时，在 Worker 的 **Settings → Domains & Routes** 添加 **Custom Domain**。[首次推送](../README.md#first-repository) · [域名配置](https://developers.cloudflare.com/workers/configuration/routing/custom-domains/)

## 部署配置

源码部署从仓库根目录运行 `mise install`、`mise run setup` 和 `mise exec -- npx wrangler login`。使用预构建包时，用包内 `wrangler.json` 和指定的 Node 版本，省略下文的 `mise exec --`。首次部署为各实例选独立的 Worker、D1、Git 存储和 R2 名称。升级时使用原资源标识。[本地部署步骤](../README.md#deploy-from-a-local-checkout)

| 方案 | Wrangler 环境 | Git 存储 | Cloudflare 资源与外部依赖 |
| --- | --- | --- | --- |
| 默认 Free | 顶层 `shard-free` | 每仓库一个 SQLite Durable Object | Workers Free、D1、Static Assets、Rate Limiting、Cron。可选 R2 |
| 托管 Paid | `managed` 或 `managed-staging` | Artifacts | Workers Paid、Artifacts、D1、R2、Containers、Durable Objects、Workflows。构建容器镜像需要 Docker |
| R2 Git（全托管大仓） | `r2` | R2 持久化 Git pack 和索引，SQLite Durable Objects 保存 refs 与清单 | Workers Paid、D1、两个私有 R2 桶、Containers、Durable Objects、Workflows。构建镜像需要 Docker |
| 旧 Artifacts | `artifacts` | 原 Artifacts 命名空间 | Workers Paid、Artifacts、D1、R2。可选自管合并桥 |
| 原生 Git（混合部署） | `native` | 自备主机持久盘上的裸 Git 仓库 | Cloudflare 托管 Worker、D1 和 R2，另需自备 HTTPS Git 源站。Paid 预设启用 R2 分片 |

这些方案均使用 `DB`、`ASSETS` 和 `REQUEST_LIMITER`。`GIT_BACKEND` 为新建仓库选择默认后端。已创建的仓库按 D1 中记录的后端访问，fork 继承源仓库的后端。配置中的变量会在发布时写入 Worker。由控制台维护的文本变量应从对应环境的 `vars` 中移除，`keep_vars: true` 会保留这些变量。令牌通过 Worker Secret 或 Git 忽略的 `.env.deploy.*` 文件提供。[配置源文件](../wrangler.jsonc) · [变量与密钥配置](../README.md#worker-variables-and-secrets)

### 默认 Free

保留顶层 `GIT_BACKEND=durable`、`SHARD_LIMIT_PROFILE=free`、`MAX_PUSH_MB=32` 和 `SHA1DC=1`。保留 `GIT_REPOS` 及其 `free-git-v1` SQLite 类迁移。点击 README 的 **Deploy to Cloudflare**，或登录后使用下面的命令。需要 LFS 时在顶层增加私有 R2 绑定 `LFS_OBJECTS`。[Free 部署步骤](../README.md#deploy-from-github-or-gitlab)

```sh
mise run deploy
```

### 托管 Paid

账号需开通 Workers Paid、Artifacts 和 Containers。保留 `GIT_BACKEND=artifacts` 和 `SHARD_LIMIT_PROFILE=paid`，为目标环境配置 `ARTIFACTS` 命名空间、`DB`、私有 `LFS_OBJECTS`、合并容器与协调器，以及复制和发布 Workflows。设置 `CLOUDFLARE_ACCOUNT_ID`。把 `SHARD_LFS_R2_ACCESS_KEY_ID`、`SHARD_LFS_R2_SECRET_ACCESS_KEY` 写入 `.env.deploy.paid`，凭据范围限定为目标桶的 Object Read & Write。预设据此配置 R2 S3 endpoint、桶名、`r2-multipart` 和 Artifacts HTTPS 源站。生产与 staging 分别使用各自的桶、凭据和绑定。[托管部署与 Workers Builds](../README.md#deploy-with-managed-cloudflare-services) · [R2 凭据](https://developers.cloudflare.com/r2/api/tokens/)

```sh
mise exec -- npm run deploy:paid -- --secrets-file .env.deploy.paid
# 独立的 staging 资源和凭据
mise exec -- npm run deploy:paid -- --env managed-staging --secrets-file .env.deploy.staging
```

### 旧 Artifacts

把既有 Worker 的名称、D1 UUID、Artifacts 命名空间、R2 桶、域名路由和可选绑定填入 `env.artifacts`。该环境采用 `paid` 处理预算，LFS 默认使用 `proxy`。需要合并分叉历史或跨仓库 PR 时，可配置 Git 2.38+ 合并桥。在 Worker 设置 `NATIVE_GIT_ORIGIN` 和密钥 `NATIVE_GIT_TOKEN`，桥接服务设置对应的 `SHARD_GIT_TOKEN` 与账号的 `SHARD_GIT_ARTIFACTS_ORIGINS`。专用合并桥可设 `SHARD_GIT_BRIDGE_ONLY=1`。升级沿用原有 Git ID 和 LFS 路径。[旧实例升级](../README.md#upgrade-an-existing-artifacts-deployment) · [Artifacts 合并桥](../README.md#web-merges-on-artifacts)

```sh
mise exec -- npm run deploy -- --env artifacts
```

### 原生 Git

Cloudflare Workers 运行 Shard 网站、API 和权限检查，D1 保存应用数据，R2 保存 LFS 对象。你在自备主机上运行 Git 源站，普通 Git 仓库落在该主机的持久盘上。`deploy:paid -- --env native` 发布 Worker 并配置 R2 分片，源站需单独运行。

在主机安装项目指定的 Node 和 Git 2.32+，Web 非快进合并需要 Git 2.38+。配置 `SHARD_GIT_DIRECTORY` 为持久卷，设置 `SHARD_GIT_TOKEN`，运行 `node scripts/git-origin.mjs`。服务默认监听 `127.0.0.1:8793`，通过 HTTPS 反代暴露。在 Worker 的 `env.native.vars` 设置 `NATIVE_GIT_ORIGIN`，并把同一份共享密钥保存为 `NATIVE_GIT_TOKEN`。普通原生部署默认采用 `free` 处理预算与 `proxy` LFS。在 Workers Paid 账号可将 `SHARD_LIMIT_PROFILE` 设为 `paid`，或使用下面的 Paid 预设。[原生源站安装](../README.md#deploy-with-a-native-git-origin)

| 启用原生直连 | 值 |
| --- | --- |
| Git 域名 | 独立 HTTPS 域名，在 Cloudflare DNS 中设为 **DNS only** 后，客户端将 pack 直接传给源站。若启用 Cloudflare HTTP 代理，上传仍受域名套餐的请求体上限约束。源站反代须流式转发请求和响应，并配置请求体积和读写空闲超时 |
| Worker 文本变量 | `NATIVE_GIT_ORIGIN` 和 `NATIVE_GIT_DIRECT_ORIGIN` 可都指向 Git 域名，例如 `https://git.example.com` |
| 源站环境变量 | `SHARD_GIT_FORGE_ORIGIN` 指向 Worker，例如 `https://forge.example.com`。源站绑定在环回接口且反代覆盖 `X-Real-IP` 时可设 `SHARD_GIT_TRUST_PROXY=1` |
| Paid 预设 | 设置 `CLOUDFLARE_ACCOUNT_ID`，绑定私有 `LFS_OBJECTS`。在密钥文件提供 `NATIVE_GIT_TOKEN`、`SHARD_LFS_R2_ACCESS_KEY_ID` 和 `SHARD_LFS_R2_SECRET_ACCESS_KEY` |

```sh
# 常规部署
mise exec -- npm run deploy -- --env native --secrets-file .env.deploy.native
# Workers Paid 与 R2 分片
mise exec -- npm run deploy:paid -- --env native --secrets-file .env.deploy.paid
```

配置直连后，从原生仓库页面复制新克隆地址。标准 Git 将 pack 传给源站。Worker 校验私库的读取授权和推送资格，源站在 Git 更新引用前再向 Worker 校验写权限与分支保护。Git LFS Batch 仍由 Worker 鉴权，R2 分片模式下客户端把对象字节直接上传到 R2。[直连反代样例](../README.md#large-repositories-on-workers-paid) · [DNS 代理状态](https://developers.cloudflare.com/dns/proxy-status/)

### R2 Git（全托管大仓）

开通 Workers Paid 和 Containers，为 `env.r2` 配置独立的 D1 数据库和两个私有桶。`GIT_PACKS` 保存普通 Git 对象，`LFS_OBJECTS` 保存 LFS 对象。为每个桶创建限于该桶的 Object Read & Write 凭据。在 `.env.deploy.r2` 中设置 `SHARD_GIT_R2_ACCESS_KEY_ID`、`SHARD_GIT_R2_SECRET_ACCESS_KEY`、`SHARD_LFS_R2_ACCESS_KEY_ID` 和 `SHARD_LFS_R2_SECRET_ACCESS_KEY`，并设置 `CLOUDFLARE_ACCOUNT_ID`。

```sh
mise exec -- npm run deploy:paid -- --env r2 --secrets-file .env.deploy.r2
```

标准 Git HTTPS 可直接使用。大 push 安装发布包中的 `git-remote-shard`，为 checkout 设置带 `shard::` 前缀的 push URL。分片传到私有 R2 暂存对象，续传列出 R2 已收到的分片。会话和最终 Git 回执保留六天。使用 `git remote-shard --list` 和 `--resume` 配合原 HTTPS 地址，恢复原来暂存的请求。分别给 `git-incoming/` 和 `lfs-incoming/` 设置七天过期和中止未完成分片上传规则。[完整命令和续传说明](../README.md#large-repositories-on-cloudflare-r2)

迁移或灾难恢复时一起保存 D1、DO 的 refs 和 pack 清单、Git pack 索引及 LFS 对象。仓库有至少 8 个 pack 时，后台可在 256 MiB 预算内整理较小的 pack 和索引。旧对象会保留 24 小时，以便已有读请求完成。[R2 Git 配套备份与恢复](../README.md#r2-git-backup-and-restore)

### 共用的 LFS 配置

`proxy` 只需私有的 `LFS_OBJECTS` 绑定。手工启用 `r2-direct` 或 `r2-multipart` 时还要设置 `SHARD_LFS_UPLOAD_MODE`、`SHARD_LFS_R2_BUCKET`、`SHARD_LFS_R2_ENDPOINT` 和上文的两项 R2 Secret。桶名与绑定一致，endpoint 与桶的账号、辖区一致。给 `lfs-incoming/` 设置 7 天过期和中止未完成 multipart 的规则。`r2-multipart` 的上传客户端需要在各 Git checkout 配置随项目分发的 `scripts/git-lfs-r2-multipart.mjs`。三种模式的下载都使用标准 Git LFS。[LFS 变量、生命周期规则和客户端命令](../README.md#git-lfs-upload-modes)

## 数量、容量与执行限制

### Cloudflare 套餐和链路

| 资源 | Workers Free | Workers Paid | 来源 |
| --- | --- | --- | --- |
| Worker 请求与 HTTP CPU | 100,000 请求/日，每个 HTTP 请求 10 ms CPU | 无日请求数量上限，HTTP 默认 30 s CPU，可设至 300 s | [Workers 限制](https://developers.cloudflare.com/workers/platform/limits/#account-plan-limits) |
| Worker 内存、子请求和并发连接 | 128 MB/隔离实例。50 外部子请求、1,000 内部服务子请求。最多 6 个同时等响应头的连接 | 同为 128 MB 和 6 个连接。子请求默认 10,000，平台允许配置至 1,000 万 | [内存与连接](https://developers.cloudflare.com/workers/platform/limits/#memory) · [子请求](https://developers.cloudflare.com/workers/platform/limits/#subrequests) |
| D1 容量与查询 | 500 MB/库、5 GB/账号、10 个库、每个 Worker 调用 50 次查询 | 10 GB/库、默认 1 TB/账号、每个 Worker 调用 1,000 次查询。账号总量可申请调整 | [D1 限制](https://developers.cloudflare.com/d1/platform/limits/) |
| D1 与 SQLite DO 的行计量 | 两个产品各自计量，每个产品每天 500 万读行、10 万写行 | 按月有包含额度，超额计费。高子请求额度仍受各存储服务的查询与吞吐限制 | [D1 计价](https://developers.cloudflare.com/d1/platform/pricing/) · [DO 计价](https://developers.cloudflare.com/durable-objects/platform/pricing/) |
| SQLite DO 存储 | 账号合计 5 GB。官方超限 FAQ 给出 Free 单对象 1 GB，通用表列为 10 GB，Free 规划采用 1 GB | 10 GB/对象，账号总存储无硬上限。两者单 SQL 行或 BLOB 最大 8 MB | [DO 存储与超限 FAQ](https://developers.cloudflare.com/durable-objects/platform/limits/#what-happens-when-a-durable-object-exceeds-its-storage-limit) |
| R2 | Standard 每月含 10 GB-month、100 万 Class A 和 1,000 万 Class B 请求。需要启用 R2 | 与 Workers 套餐分别计量。R2 文档列出桶总存储与对象数无硬上限，对象与写入操作有独立限制 | [R2 计价](https://developers.cloudflare.com/r2/pricing/) · [R2 限制](https://developers.cloudflare.com/r2/platform/limits/) |

使用 Cloudflare HTTP 代理入口时，Free／Pro **域名套餐**每个请求体最大 100 MB，Business 200 MB，Enterprise 可自行配置至 5 GB。该入口对流式上传同样执行体积检查。Durable Object `fetch()` 向外部服务发包和经 Container outbound handler 的 HTTP 请求体分别还有默认 100 MiB 的限制。[入口体积](https://developers.cloudflare.com/workers/platform/limits/#request-and-response-limits) · [DO 出站](https://developers.cloudflare.com/durable-objects/platform/limits/#outbound-http-request-body-size) · [Container 出站](https://developers.cloudflare.com/containers/platform/limits/#outbound-http-request-body-size)

HTTP Worker 可以在客户端连接存续时流式运行，运行时更新可在 20 分钟排空窗口后关闭旧连接，`waitUntil()` 最多延长 30 秒。若另有 Cloudflare 反代源站链路，默认 proxy read/write 超时分别为 125 秒与 30 秒。[Worker 执行时长](https://developers.cloudflare.com/workers/platform/limits/#duration) · [反代连接限制](https://developers.cloudflare.com/fundamentals/reference/connection-limits/)

### Shard 的 Git 与后台预算

| 项目 | 交付配置与容量边界 |
| --- | --- |
| Free Git | `MAX_PUSH_MB` 可设 1–32 MiB，默认 32 MiB。push 计入 receive 命令及 pack。fork、公开 HTTPS import 需要把快照装进一个同样大小的 pack，复制最多 2,048 refs 和 1 MiB 引用元数据，导入期限 60 s。`INGEST_CACHE_MB` 默认 16 MiB，调小缓存可给其他解析留内存。展开的对象、delta 解析与 SQLite 索引仍占资源。[SQLite Git 实现](../worker/git-repository.mjs) |
| Artifacts Git | 每次 push 64 MiB、50,000 个对象、1 MiB 的引用更新，单文件 32 MiB。创建于 2026-09-29 起的仓库为 1 GB，此前仓库为 10 GB。账号总存储默认 1 TB，可申请提高。每命名空间的控制面和每仓库的 Git 请求各限 2,000 次/10 s。[Artifacts 限制](https://developers.cloudflare.com/artifacts/platform/limits/) |
| R2 Git | 普通 Git 数据保存为不可变 pack 和索引。`SHARD_GIT_R2_MAX_PUSH_BYTES` 默认 `2 GiB`，配置上限为 `5,492,189,429,760` 字节。标准 HTTPS push 还受域名请求体限制。分片至少 32 MiB，最多 10,000 片。refs 与 pack 清单共用 4 MiB 工作区元数据预算，仓库吞吐还受容器工作盘和 R2 读取影响。[R2 Git 实现](../worker/git-r2-repository.mjs) · [R2 限制](https://developers.cloudflare.com/r2/platform/limits/) |
| 原生 Git | 经 Worker 代理的 URL 仍遵守入口限制。直连 pack 使用源站的 Git、文件系统、磁盘、网络和反代资源。保留引用与鉴权控制请求的独立预算。同一仓库的写操作串行。[原生源站](../scripts/git-origin.mjs) · [Git receive 配置](https://git-scm.com/docs/git-config#Documentation/git-config.txt-receivemaxInputSize) |
| Git 控制请求 | 经 Worker 代理的 receive 命令前缀上限为 256 KiB。直连 Git 的引用钩子输入上限为 256 KiB，发往 Worker 的控制 JSON 上限为 512 KiB。Shard 的限流配置默认按请求类别与来源 IP 使用 300 次/60 s，由 Cloudflare 各位置近似计数。[Git 协议解析](../src/git_wire.js) · [直连授权](../src/handlers/git.rs) · [Rate Limiting](https://developers.cloudflare.com/workers/runtime-apis/bindings/rate-limit/) |
| Paid 执行配置 | `managed`、`managed-staging`、`r2` 和 `deploy:paid -- --env native` 默认 300 s CPU、20,000 子请求。旧 Artifacts 配置为 300 s CPU，子请求继承 Workers Paid 默认值。普通 `native` 使用 `free` 预设，将其显式切到 `paid` 时部署脚本会设置 300 s CPU，支持 `limits` 覆盖。[Wrangler 配置](../wrangler.jsonc) · [部署脚本](../scripts/deploy.mjs) |
| 搜索和 PR | `free`/`paid` 的代码搜索各扫描最多 2,048/4,096 条树项及 512 KiB/2 MiB 文件内容，提交搜索读取 24/64 个首父提交。Paid 快照 PR 比较最多读取 160 棵树及检查 8,192 条树项。原生同源比较限 5 s 和 256 个变更文件。大仓传输与单页搜索、diff 是两种独立预算。[搜索](../src/repo_search.rs) · [差异比较](../src/pull_diff.rs) · [原生 Git 比较](../scripts/git-origin.mjs) |
| Cron 与 Workflow | 分钟 Cron 的 `free`/`paid` 预算各为 1/24 个未完成推送、1/12 个合并队列条目、每类 1/32 个 webhook。Paid 配置启用 S3 时将每轮复制检查从 12 降到 3，每轮 PR 刷新从 32 降到 28。托管复制 Workflow 每步最多拷贝一页 LFS，每页 8 个对象。实例最多推进 120 个复制步骤，剩余任务由 Cron 恢复。分钟 Cron 的平台墙钟上限 15 分钟，实际处理量还受 D1/R2 与 CPU 约束。[后台预算](../src/limits.rs) · [复制 Workflow](../worker/repository-copy.mjs) · [Workflows 限制](https://developers.cloudflare.com/workflows/reference/limits/) |
| 托管 Git 合并容器 | Shard 默认 `standard-1`，0.5 vCPU、4 GiB RAM、8 GB 盘。`SHARD_GIT_MERGE_INSTANCE` 可选具名规格，`standard-4` 为 4 vCPU、12 GiB RAM、20 GB 盘。容器存储是临时工作区，备份快照有镜像版本绑定、20 GB 上限和 30 天保留约束。[容器实例规格](https://developers.cloudflare.com/containers/platform/limits/#instance-types) · [快照](https://developers.cloudflare.com/containers/guides/snapshots/) |

### LFS 对象上限

| `SHARD_LFS_UPLOAD_MODE` | `free` 处理预算 | `paid` 处理预算 | 数据路径 |
| --- | --- | --- | --- |
| `proxy` | 默认及最高 100,000,000 字节，可调低 | 默认 100,000,000 字节，可提高到域名请求上限与 5,363,466,240 字节中的较低值 | Worker 流式转发到 R2 |
| `r2-direct` | 5,363,466,240 字节 | 5,363,466,240 字节 | 客户端用预签名 URL 单次 PUT 到 R2，之后完成校验和发布 |
| `r2-multipart` | 默认及最高 5,363,466,240 字节 | 默认 5,363,466,240 字节。显式配置最高 5,492,189,429,760 字节，约 4.995 TiB | 客户端分片直传 R2，可在 6 天内从已登记分片续传。单对象最多 10,000 片 |

用 `SHARD_LFS_MAX_BYTES` 按字节设置实例上限。已发布对象仍可按原权限下载。升级前若有超出新默认值的未完成上传，可显式设置对应上限后续传。LFS Batch 单次最多 100 个对象。超过单 PUT 上限时，Worker 会读取 R2 暂存对象做完整 SHA-256，再通过 S3 multipart copy 发布。完整校验与发布各有 15 分钟的窗口，且共享一次调用的 CPU 额度。4.995 TiB 在 15 分钟内读完，仅 I/O 就需要约 6.10 GB/s 的持续吞吐。提高上限前应按目标实例测量校验、发布与 fork 复制。[Shard LFS 配置](../src/limits.rs) · [R2 分片与完整校验](../src/lfs_remote.rs) · [R2 对象限制](https://developers.cloudflare.com/r2/platform/limits/)

## 分片容量、耗时与并发

R2 Git 和 LFS 使用相同的分片尺寸算法。设上传字节数为 `B`，每片为 `max(32, ceil(B / (10,000 × 1 MiB))) MiB`，片数为 `ceil(B / 每片字节数)`。末片可更小。对普通 Git，`B` 是整个 receive-pack 请求体，包含打包压缩后的对象与 Git 命令。对 LFS，`B` 是文件的原始字节数。[Git 分片计划](../worker/git-r2-repository.mjs) · [LFS 分片计划](../src/lfs_remote.rs) · [R2 Multipart 规则](https://developers.cloudflare.com/r2/objects/upload-objects/)

| 上传体积 | 每片 | 片数 | 持续 100 Mb/s | 持续 500 Mb/s |
| --- | ---: | ---: | ---: | ---: |
| 129 MiB | 32 MiB | 5 | 11 秒 | 3 秒 |
| 1 GiB | 32 MiB | 32 | 86 秒 | 18 秒 |
| 2 GiB | 32 MiB | 64 | 172 秒 | 35 秒 |
| 4 GiB | 32 MiB | 128 | 344 秒 | 69 秒 |
| 8 GiB | 32 MiB | 256 | 688 秒 | 138 秒 |
| 16 GiB | 32 MiB | 512 | 1,375 秒 | 275 秒 |
| R2 上限 5,115 GiB | 524 MiB | 9,996 | 约 122.0 小时 | 约 24.4 小时 |

时间按 `B × 8 / 链路比特率` 向上取整，是只传字节的下界。Git 还要在客户端生成并暂存 pack、在容器验证并写入 pack 和索引、随后发布 refs。LFS 还要校验并发布完整对象。设置 `SHARD_GIT_R2_MAX_PUSH_BYTES` 时，从 2 GiB 默认值和 `SHARD_GIT_R2_INSTANCE=standard-4` 的 20 GB 临时盘出发，按峰值工作盘和完成耗时逐级验证。设置 `SHARD_LFS_MAX_BYTES` 时，对超过单 PUT 上限的样本还要测 Worker 全量哈希、发布和 fork。两者的 R2 字节上限描述协议和配置可接受的范围。[R2 Git 工作区](../scripts/git-r2-origin.mjs) · [LFS 校验与发布](../src/lfs_remote.rs) · [容器规格](https://developers.cloudflare.com/containers/platform/limits/#instance-types)

### 单次 PUT 和 multipart 的选择

两种 R2 上传都绕过 Worker 的请求体入口。LFS `r2-direct` 对稳定链路上的中小文件有更少的控制请求，失败时重传整个文件。multipart 多了初始化、逐片签名与最终提交，大文件中断时可以只补缺失片。比如 8 GiB 的上传有 256 片，已经确认 200 片时，续传只需 56 片，即 1.75 GiB。R2 支持同时上传不同编号的分片。当前两个 Shard 客户端对同一个对象均按顺序上传。[R2 上传方式](https://developers.cloudflare.com/r2/objects/upload-objects/) · [Git helper](../scripts/git-remote-shard.mjs) · [LFS 适配器](../scripts/git-lfs-r2-multipart.mjs)

### 并发能加速哪些阶段

- **多个 LFS 对象**已经可以并行。Git LFS 默认可启动多个适配器进程，可用 `git config --local lfs.concurrenttransfers 4` 显式设为四个并发传输。为共享公网出口的客户端预留容量，复查整体吞吐、限流响应和校验耗时。[Git LFS 配置](https://github.com/git-lfs/git-lfs/blob/main/docs/man/git-lfs-config.adoc#upload-and-download-transfer-settings) · [自定义传输](https://github.com/git-lfs/git-lfs/blob/main/docs/custom-transfers.md#defining-a-custom-transfer-type)
- **单个 Git 或 LFS 对象**可以在客户端增加有界的异步 PUT 队列。用同一文件的独立字节范围试测并发 1、2、4、8。每片独立重试和刷新签名，进度按片聚合，收到全部分片后进入完成步骤。Git 每片需要向 Worker 签名，LFS 还需登记 ETag。并发总量受上行带宽和按来源 IP 的 Git 请求限流约束。Git 仍需按仓库顺序发布引用，LFS 仍需校验整个对象。[Git 上传计划](../worker/git-r2-repository.mjs) · [LFS 分片登记](../src/lfs_remote.rs) · [限流配置](../wrangler.jsonc)
- **Git 计算**是独立阶段。`pack.threads` 影响 pack 的 delta 搜索与内存用量。R2 Git 容器工作区设为 1 线程，容器将新 pack 写入 R2 时 rclone 已设 2 路分片并发。分开测客户端打包、上传、容器收包和对象发布，才能定位值得加并发的阶段。[Git pack 配置](https://git-scm.com/docs/git-config#Documentation/git-config.txt-packthreads) · [容器 Git 配置](../scripts/git-origin.mjs) · [容器 R2 上传](../scripts/git-r2-origin.mjs)

## 各方案可以继续提高哪些上限

### 默认 Free

启用 R2 后，`r2-direct` 可扩大单个 LFS 文件的上传容量。按仓库划分的 Durable Objects 分散仓库之间的并发。限制 delta 缓存可留出解析内存，对元数据查询做索引、分页和合批可节省 D1 的日读写额度。[Free Git 实现](../worker/git-repository.mjs) · [D1 索引](https://developers.cloudflare.com/d1/best-practices/use-indexes/)

`MAX_PUSH_MB=32` 已到 Free 后端当前的实现上限。历史可以分段时，分多次推送可装入更小的 pack，累计数据仍占单仓库 SQLite 配额。需要更大的单次推送或仓库容量时，可按实际负载评估 [R2 Git](#r2-git-全托管大仓) 或原生直连。[DO 限制](https://developers.cloudflare.com/durable-objects/platform/limits/) · [Workers 限制](https://developers.cloudflare.com/workers/platform/limits/)

### 托管 Paid

可调高 `SHARD_GIT_MERGE_INSTANCE` 给 Web 合并分配更多 CPU、内存和工作盘。按仓库和目标分支分散合并协调，可提高多仓吞吐。提高 `limits.subrequests` 或增加 Workflow 页数前，先看每步 CPU、D1 调用、R2 请求、复制耗时和续跑表现。每页 8 个接近上限的对象可产生 8,192 次 S3 分段复制请求，批量需为其它调用和故障恢复留余量。[Containers 规格](https://developers.cloudflare.com/containers/platform/limits/) · [Workflows 限制](https://developers.cloudflare.com/workflows/reference/limits/) · [复制实现](../src/lfs_s3.js)

要进一步提高可用 LFS 容量，可将完整校验和条件发布拆成可恢复的后台步骤。由容器对指定版本的 R2 对象做流式校验，Worker 再按暂存对象版本发布，可把全量哈希移到更大的计算预算下。Workflow 持久保存对象标识和进度。[Workflow 持久执行](https://developers.cloudflare.com/workflows/build/rules-of-workflows/) · [当前 LFS 发布](../src/lfs_remote.rs)

普通 Git 的 push、单文件及仓库额度由 Artifacts 执行。增加容器规格扩大合并能力，申请账号容量扩大多个仓库的总存储。需要更大单次推送或单仓库容量时，可选 R2 Git 或原生直连。[Artifacts 限制](https://developers.cloudflare.com/artifacts/platform/limits/) · [R2 Git 部署](../README.md#large-repositories-on-cloudflare-r2)

### 旧 Artifacts

使用现有 R2 桶可切换到签名直传或分片。保留 S3 配置可支持已有大 LFS 对象的 fork 复制。补配外部合并桥可支持更多 Web 合并。后台吞吐需要继续扩大时，可为该实例引入托管方案的分支协调与复制 Workflow，在独立 staging 验证资源配置。[旧部署升级](../README.md#upgrade-an-existing-artifacts-deployment) · [LFS 上传模式](../README.md#git-lfs-upload-modes)

保留 Artifacts 命名空间和仓库 ID 可保留早期仓库的 10 GB 容量。单次 push 执行与托管 Paid 相同的服务限制。迁移到其他 Git 后端时，先暂停写入，核对 refs、历史 PR 所需对象和 LFS 内容，再变更存储映射。[Artifacts 容量变更](https://developers.cloudflare.com/changelog/post/2026-09-29-repository-size-limit/) · [存储变更流程](../PLAN.md#optional-git-backends)

### R2 Git

容器用临时盘生成新 pack，按需从只读挂载读取 R2 上已有的 pack 和索引。SQLite Durable Object 只在新对象已存入 R2、权限和 refs 复查通过后发布清单。仓库可以累计保存大于单次 push 或容器工作盘的数据。单次接收、导入或合并仍需足够的工作盘、内存和执行时间。备份与恢复应同时覆盖 R2 pack、DO refs 和 D1。再提高配置前，先测冷启动读取、Git 对象校验、克隆和 fork。[容器磁盘行为](https://developers.cloudflare.com/containers/faq/#is-disk-persistent-what-happens-to-my-disk-when-my-container-sleeps) · [R2 Git 工作区](../scripts/git-r2-origin.mjs)

### 原生 Git

原生直连可随源站扩容。扩大持久卷，为 Git unpack、clone 和 fork 预留 CPU 与临时盘，按吞吐调整反向代理的请求体和读写超时。Git 的 `receive.maxInputSize` 可作为源站接收入包的运营上限，省略或设为 `0` 使用 Git 的默认行为。当前以实例为单位配置源站、磁盘和 D1，可部署多个独立实例分担仓库。同一仓库的写入仍需维持引用锁，跨源站分片与迁移需要额外实现。[Git receive 配置](https://git-scm.com/docs/git-config#Documentation/git-config.txt-receivemaxInputSize) · [源站实现](../scripts/git-origin.mjs) · [直连部署](../README.md#large-repositories-on-workers-paid)

Workers Paid 可增加 Web 搜索和后台任务预算。原生同源比较使用源站计算，提交快照与分页仍限制单次网页查询的工作量。大 LFS 上传可使用 R2 分片，实际可用容量取决于完整校验和复制时窗。按预期负载分别记录源站磁盘和 Git 打包、Worker CPU、D1 查询及 R2 完成阶段的数据。[后台与 LFS 预算](../src/limits.rs) · [原生比较和文件读取](../scripts/git-origin.mjs)

## 已有验收与发布前验证

本地使用标准 Git 在直连的私库一次推送了 128 MiB 不可压缩数据，落盘 pack 超过 128 MiB，经克隆校验提交 ID、对象大小和 `git fsck --full`。保护分支和推送中撤销写权限均阻止了引用更新。Free 用例推送并复制了含 12 MiB 不可压缩数据的快照，fork 在删除源仓库并重启后仍可完整克隆。R2 模拟器验证了私有暂存、签名 PUT 和分片续传。原生 Paid 已通过 Wrangler dry-run。以上为本地验证，生产 HTTPS 源站、Cloudflare 托管资源和接近 R2 协议上限的传输还需要按目标负载测量。[验证记录](../STATUS.md#verification-log)

R2 Git 已在本地 Docker 和 Miniflare 验证 129 MiB 不可压缩文件的分片推送。Worker 重启后只补传缺失分片，发布时重新检查分支保护，重复完成请求返回同一份 Git 回执。冷克隆通过 `git fsck --full`。本地中转补齐模拟器的 `ListParts` 并适配 ETag 解析，Cloudflare 云端仍按目标负载验证。 [R2 Git 端到端测试](../tests/r2-git.test.mjs)

部署前，用对应环境执行 `--dry-run` 核对绑定及变量，Paid 预设的 dry-run 可省略密钥文件。上线前在独立实例使用正常的 HTTPS 域名验证一次 push、clone 和 `git fsck --full`，并验证私库、只读令牌与受保护分支。准备使用大 LFS 时测上传、断点恢复、完整校验、下载和 fork 复制，分别记录对象大小、各阶段耗时与账单计量。选择初始 `SHARD_LFS_MAX_BYTES` 时同时考虑校验和 fork 的完成率。[部署命令](../README.md#deploy-from-a-local-checkout) · [R2 监控](https://developers.cloudflare.com/r2/platform/metrics-analytics/)
