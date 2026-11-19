# Shard 部署配置、资源限制与扩容空间

选择部署时先确认需要扩大的是单次 Git push、整个仓库、单个 LFS 对象，还是搜索、合并和后台作业的吞吐。Workers 套餐决定计算和服务额度，域名套餐决定经 Cloudflare 入口的单请求体积，Git 存储后端决定 pack 和仓库容量。下文使用 Cloudflare 原文的 MB、GB 与 MiB、GiB，两个单位体系分别按十进制和二进制计量。[Workers 限制](https://developers.cloudflare.com/workers/platform/limits/) · [Artifacts 限制](https://developers.cloudflare.com/artifacts/platform/limits/)

## 部署配置

从仓库根目录运行命令。源码部署使用 `mise install`、`mise run setup` 和 `mise exec -- npx wrangler login`。预构建包使用包内 `wrangler.json` 和指定的 Node 版本，在 `npm ci` 后运行对应命令，省略 `mise exec --`。部署脚本解析目标环境的 `DB`，应用迁移后发布 Worker。首次部署为各实例选独立的 Worker、D1、Git 存储和 R2 名称。升级既有实例时使用它原来的资源标识。[本地部署步骤](../README.md#deploy-from-a-local-checkout) · [预构建发布包](../README.md#project-releases)

| 方案 | Wrangler 环境 | Git 存储 | Cloudflare 资源与外部依赖 |
| --- | --- | --- | --- |
| 默认 Free | 顶层 `shard-free` | 每仓库一个 SQLite Durable Object | Workers Free、D1、Static Assets、Rate Limiting、Cron。可选 R2 |
| 托管 Paid | `managed` 或 `managed-staging` | Artifacts | Workers Paid、Artifacts、D1、R2、Containers、Durable Objects、Workflows。构建容器镜像需要 Docker |
| 旧 Artifacts | `artifacts` | 原 Artifacts 命名空间 | Workers Paid、Artifacts、D1、R2。可选自管合并桥 |
| 原生 Git | `native` | 自管主机上的裸 Git 仓库 | Workers Free 或 Paid、D1、R2、HTTPS Git 源站及持久盘。Paid 预设启用 R2 分片 |

四种方案均使用 `DB`、`ASSETS` 和 `REQUEST_LIMITER`。`GIT_BACKEND` 为新建仓库和无凭据导入选择默认后端。已创建的仓库按 D1 中记录的后端访问，fork 继承源仓库的后端。配置中的变量会在发布时写入 Worker。由控制台维护的文本变量应从对应环境的 `vars` 中移除，`keep_vars: true` 会保留这些变量。令牌通过 Worker Secret 或 Git 忽略的 `.env.deploy.*` 文件提供。[配置源文件](../wrangler.jsonc) · [变量与密钥配置](../README.md#worker-variables-and-secrets)

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

在主机安装项目指定的 Node 和 Git 2.32+，Web 非快进合并需要 Git 2.38+。配置 `SHARD_GIT_DIRECTORY` 为持久卷，设置 `SHARD_GIT_TOKEN`，运行 `node scripts/git-origin.mjs`。服务默认监听 `127.0.0.1:8793`，通过 HTTPS 反代暴露。在 Worker 的 `env.native.vars` 设置 `NATIVE_GIT_ORIGIN`，并把同一份共享密钥保存为 `NATIVE_GIT_TOKEN`。普通原生部署默认采用 `free` 处理预算与 `proxy` LFS。在 Workers Paid 账号可将 `SHARD_LIMIT_PROFILE` 设为 `paid`，或使用下面的 Paid 预设。[原生源站安装](../README.md#deploy-with-a-native-git-origin)

| 启用原生直连 | 值 |
| --- | --- |
| Git 域名 | HTTPS，DNS only。反代流式转发请求和响应，设置所需的请求体积和读写空闲超时 |
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
| 原生 Git | 经 Worker 代理的 URL 仍遵守入口限制。直连 pack 使用源站的 Git、文件系统、磁盘、网络和反代资源。保留引用与鉴权控制请求的独立预算。同一仓库的写操作串行。[原生源站](../scripts/git-origin.mjs) · [Git receive 配置](https://git-scm.com/docs/git-config#Documentation/git-config.txt-receivemaxInputSize) |
| Git 控制请求 | 经 Worker 代理的 receive 命令前缀上限为 256 KiB。直连 Git 的引用钩子输入上限为 256 KiB，发往 Worker 的控制 JSON 上限为 512 KiB。Shard 的限流配置默认按请求类别与来源 IP 使用 300 次/60 s，由 Cloudflare 各位置近似计数。[Git 协议解析](../src/git_wire.js) · [直连授权](../src/handlers/git.rs) · [Rate Limiting](https://developers.cloudflare.com/workers/runtime-apis/bindings/rate-limit/) |
| Paid 执行配置 | 托管和 `deploy:paid -- --env native` 默认 300 s CPU、20,000 子请求。旧 Artifacts 配置为 300 s CPU，子请求继承 Workers Paid 默认值。普通 `native` 使用 `free` 预设，将其显式切到 `paid` 时部署脚本会设置 300 s CPU，支持 `limits` 覆盖。[Wrangler 配置](../wrangler.jsonc) · [部署脚本](../scripts/deploy.mjs) |
| 搜索和 PR | `free`/`paid` 的代码搜索各扫描最多 2,048/4,096 条树项及 512 KiB/2 MiB 文件内容，提交搜索读取 24/64 个首父提交。Paid 快照 PR 比较最多读取 160 棵树及检查 8,192 条树项。原生同源比较限 5 s 和 256 个变更文件。大仓传输与单页搜索、diff 是两种独立预算。[搜索](../src/repo_search.rs) · [差异比较](../src/pull_diff.rs) · [原生 Git 比较](../scripts/git-origin.mjs) |
| Cron 与 Workflow | 分钟 Cron 的 `free`/`paid` 预算各为 1/24 个未完成推送、1/12 个合并队列条目、每类 1/32 个 webhook。Paid 配置启用 S3 时将每轮复制检查从 12 降到 3，每轮 PR 刷新从 32 降到 28。托管复制 Workflow 每步最多拷贝一页 LFS，每页 8 个对象。实例最多推进 120 个复制步骤，剩余任务由 Cron 恢复。分钟 Cron 的平台墙钟上限 15 分钟，实际处理量还受 D1/R2 与 CPU 约束。[后台预算](../src/limits.rs) · [复制 Workflow](../worker/repository-copy.mjs) · [Workflows 限制](https://developers.cloudflare.com/workflows/reference/limits/) |
| 托管 Git 合并容器 | Shard 默认 `standard-1`，0.5 vCPU、4 GiB RAM、8 GB 盘。`SHARD_GIT_MERGE_INSTANCE` 可选具名规格，`standard-4` 为 4 vCPU、12 GiB RAM、20 GB 盘。容器存储是临时工作区，备份快照有镜像版本绑定、20 GB 上限和 30 天保留约束。[容器实例规格](https://developers.cloudflare.com/containers/platform/limits/#instance-types) · [快照](https://developers.cloudflare.com/containers/guides/snapshots/) |

### LFS 对象上限

| `SHARD_LFS_UPLOAD_MODE` | `free` 处理预算 | `paid` 处理预算 | 数据路径 |
| --- | --- | --- | --- |
| `proxy` | 默认及最高 100,000,000 字节，可调低 | 默认 100,000,000 字节，可提高到域名请求上限与 5,363,466,240 字节中的较低值 | Worker 流式转发到 R2 |
| `r2-direct` | 5,363,466,240 字节 | 5,363,466,240 字节 | 客户端用预签名 URL 单次 PUT 到 R2，之后完成校验和发布 |
| `r2-multipart` | 5,363,466,240 字节 | 5,492,189,429,760 字节，约 4.995 TiB | 客户端分片直传 R2，可在 6 天内从已登记分片续传。单对象最多 10,000 片 |

用 `SHARD_LFS_MAX_BYTES` 按字节设置上表范围内的实例上限。LFS Batch 单次最多 100 个对象。对象超过单 PUT 上限时，Worker 会读取 R2 暂存对象做完整 SHA-256，然后通过 S3 multipart copy 发布。完整校验与发布各有 15 分钟的当前实现窗口，且共享一次调用的 CPU 额度。4.995 TiB 在 15 分钟内读完，仅 I/O 就需要约 6.10 GB/s 的持续吞吐。实际上线值按校验与 fork 复制测得的能力设定。[Shard LFS 配置](../src/limits.rs) · [R2 分片与完整校验](../src/lfs_remote.rs) · [R2 对象限制](https://developers.cloudflare.com/r2/platform/limits/)

## 各方案可以继续提高哪些上限

### 默认 Free

启用 R2 后，`r2-direct` 可扩大单个 LFS 文件的上传容量。按仓库划分的 Durable Objects 分散仓库之间的并发。限制 delta 缓存可留出解析内存，对元数据查询做索引、分页和合批可节省 D1 的日读写额度。[Free Git 实现](../worker/git-repository.mjs) · [D1 索引](https://developers.cloudflare.com/d1/best-practices/use-indexes/)

`MAX_PUSH_MB=32` 已到当前实现的最高值。历史可以分段时，分多次推送可装入更小的 pack，累计数据仍占单仓库 SQLite 配额。将不可变 pack 放进 R2，让 DO 管理引用与索引，可把主要 Git 字节移出 SQLite。采用这条路线需要实现跨存储提交顺序、崩溃恢复和 fork，并验证索引容量、解压 CPU 和免费日额度。[DO 限制](https://developers.cloudflare.com/durable-objects/platform/limits/) · [Workers 限制](https://developers.cloudflare.com/workers/platform/limits/)

### 托管 Paid

可调高 `SHARD_GIT_MERGE_INSTANCE` 给 Web 合并分配更多 CPU、内存和工作盘。按仓库和目标分支分散合并协调，可提高多仓吞吐。提高 `limits.subrequests` 或增加 Workflow 页数前，先看每步 CPU、D1 调用、R2 请求、复制耗时和续跑表现。每页 8 个接近上限的对象可产生 8,192 次 S3 分段复制请求，批量需为其它调用和故障恢复留余量。[Containers 规格](https://developers.cloudflare.com/containers/platform/limits/) · [Workflows 限制](https://developers.cloudflare.com/workflows/reference/limits/) · [复制实现](../src/lfs_s3.js)

要进一步提高可用 LFS 容量，可将完整校验和条件发布拆成可恢复的后台步骤。由容器对指定版本的 R2 对象做流式校验，Worker 再按暂存对象版本发布，可把全量哈希移到更大的计算预算下。Workflow 持久保存对象标识和进度。[Workflow 持久执行](https://developers.cloudflare.com/workflows/build/rules-of-workflows/) · [当前 LFS 发布](../src/lfs_remote.rs)

普通 Git 的 push、单文件及仓库额度由 Artifacts 执行。增加容器规格扩大合并能力，申请账号容量扩大多个仓库的总存储。需要更大单次推送或单仓库容量时，使用原生直连，或另行设计可持久化的 Git 数据服务。容器的临时盘适合计算，快照和 R2 FUSE 有各自的持久化和文件系统约束。持续写入裸仓库需要可靠的引用锁和原子更新。[Artifacts 限制](https://developers.cloudflare.com/artifacts/platform/limits/) · [容器磁盘行为](https://developers.cloudflare.com/containers/faq/#is-disk-persistent-what-happens-to-my-disk-when-my-container-sleeps) · [R2 FUSE](https://developers.cloudflare.com/containers/examples/r2-fuse-mount/)

### 旧 Artifacts

使用现有 R2 桶可切换到签名直传或分片。保留 S3 配置可支持已有大 LFS 对象的 fork 复制。补配外部合并桥可支持更多 Web 合并。后台吞吐需要继续扩大时，可为该实例引入托管方案的分支协调与复制 Workflow，在独立 staging 验证资源配置。[旧部署升级](../README.md#upgrade-an-existing-artifacts-deployment) · [LFS 上传模式](../README.md#git-lfs-upload-modes)

保留 Artifacts 命名空间和仓库 ID 可保留早期仓库的 10 GB 容量。单次 push 执行与托管 Paid 相同的服务限制。迁移到其他 Git 后端时，先暂停写入，核对 refs、历史 PR 所需对象和 LFS 内容，再变更存储映射。[Artifacts 容量变更](https://developers.cloudflare.com/changelog/post/2026-09-29-repository-size-limit/) · [存储变更流程](../PLAN.md#optional-git-backends)

### 原生 Git

原生直连可随源站扩容。扩大持久卷，为 Git unpack、clone 和 fork 预留 CPU 与临时盘，按吞吐调整反向代理的请求体和读写超时。Git 的 `receive.maxInputSize` 可作为源站接收入包的运营上限，省略或设为 `0` 使用 Git 的默认行为。当前以实例为单位配置源站、磁盘和 D1，可部署多个独立实例分担仓库。同一仓库的写入仍需维持引用锁，跨源站分片与迁移需要额外实现。[Git receive 配置](https://git-scm.com/docs/git-config#Documentation/git-config.txt-receivemaxInputSize) · [源站实现](../scripts/git-origin.mjs) · [直连部署](../README.md#large-repositories-on-workers-paid)

Workers Paid 可增加 Web 搜索和后台任务预算。原生同源比较使用源站计算，提交快照与分页仍限制单次网页查询的工作量。大 LFS 上传可使用 R2 分片，实际可用容量取决于完整校验和复制时窗。按预期负载分别记录源站磁盘和 Git 打包、Worker CPU、D1 查询及 R2 完成阶段的数据。[后台与 LFS 预算](../src/limits.rs) · [原生比较和文件读取](../scripts/git-origin.mjs)

## 已有验收与发布前验证

本地使用标准 Git 在直连的私库一次推送了 128 MiB 不可压缩数据，落盘 pack 超过 128 MiB，经克隆校验提交 ID、对象大小和 `git fsck --full`。保护分支和推送中撤销写权限均阻止了引用更新。Free 用例推送并复制了含 12 MiB 不可压缩数据的快照，fork 在删除源仓库并重启后仍可完整克隆。R2 模拟器验证了私有暂存、签名 PUT 和分片续传。原生 Paid 已通过 Wrangler dry-run。以上为本地验证，生产 HTTPS 源站、Cloudflare 托管资源和接近 R2 协议上限的传输还需要按目标负载测量。[验证记录](../STATUS.md#verification-log)

部署前，用对应环境执行 `--dry-run` 核对绑定及变量，Paid 预设的 dry-run 可省略密钥文件。上线前在独立实例使用正常的 HTTPS 域名验证一次 push、clone 和 `git fsck --full`，并验证私库、只读令牌与受保护分支。准备使用大 LFS 时测上传、断点恢复、完整校验、下载和 fork 复制，分别记录对象大小、各阶段耗时与账单计量。选择初始 `SHARD_LFS_MAX_BYTES` 时同时考虑校验和 fork 的完成率。[部署命令](../README.md#deploy-from-a-local-checkout) · [R2 监控](https://developers.cloudflare.com/r2/platform/metrics-analytics/)
