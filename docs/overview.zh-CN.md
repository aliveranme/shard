# Shard 项目说明

Shard 是一个部署在 Cloudflare 上的自托管 Git 协作平台。它把仓库、代码浏览、
议题、拉取请求、审查、发布、通知和 CI 结果放在同一个 Worker 应用里，同时让
标准 Git 客户端通过 HTTPS 完成 clone、fetch 和 push。

本文说明项目目标、运行架构、Git 数据路径、安全模型、部署选择、容量边界和
当前状态。部署步骤见 [README](../README.md#deploy-to-cloudflare) 和
[Cloudflare Dashboard 中文配置手册](configuration.zh-CN.md)；容量分析见
[部署容量指南](deployment-capacity.md)；逐项验证结果见 [STATUS.md](../STATUS.md)。

## 1. 定位与边界

Shard 面向个人项目和小团队，目标是用 Cloudflare 托管一个可用的 Git forge，
而不是复刻完整的 GitHub 或 GitLab：

- **Git 是事实来源**。提交、树、blob、标签和分支引用由 Git 存储层保存；
  D1 只保存 forge 元数据、权限和协作状态。
- **标准协议优先**。不要求用户安装专用客户端。普通 HTTPS 远程即可读取、
  拉取和推送；只有超过域名请求体上限的推送才需要 `git-remote-shard`。
- **Cloudflare 原生**。默认部署不依赖 VPS；D1、Durable Objects、R2、
  Containers、Workflows 都按需使用。
- **可替换存储后端**。同一套 Web 与权限模型可以落在 SQLite Durable Object、
  Artifacts、R2 pack 或已有的原生 Git 源站上。
- **不做的事**：不提供完整的组织计费、代码搜索索引、跨实例联邦、任意 CI
  执行环境或 Git 协议的全部传输形式（如 SSH）。仓库搜索只扫描一个有界快照。

## 2. 功能概览

| 领域 | 能力 |
|---|---|
| 账户与安全 | 注册、登录限流、密码修改、账户删除、邮箱验证与重置、会话、CSRF、同源检查、CSP |
| 仓库 | 公开/私有仓库、协作者、组织团队、重命名、转移、分叉、私有 HTTPS 导入 |
| 代码浏览 | 分支/标签、目录、文件、提交历史、父提交、原始文件、提交固定源码 TAR |
| 拉取请求 | 分支或跨仓比较、文件 diff、行评论、审查状态、保护分支、合并队列 |
| 议题 | 标签、指派、里程碑、模板、反应、筛选、讨论搜索、通知收件箱 |
| 发布 | 基于 Git 标签的草稿与发布、发布说明 |
| 集成 | 签名 Webhook、CI 上下文与结果、分支更新和标签事件 |
| Git | Smart HTTP、token 认证、分支/HEAD 管理、LFS、大推送续传 |
| 签名 | 账户 GPG 公钥、提交签名验证与邮箱验证状态 |
| 界面 | 服务端渲染、中英文、响应式主题、减少动态效果支持 |

## 3. 运行架构

```text
Browser / Git client
        │  HTTPS
        ▼
Rust Worker (src/, worker/index.mjs)
  ├─ Web 页面、REST/JSON、认证、权限、CSRF、限流
  ├─ Git Smart HTTP 入口与仓库解析
  ├─ D1：账户、权限、议题、PR、通知、任务、Webhook
  ├─ 静态资源：Static Assets
  └─ 按仓库路由到 Git 后端
        │
        ├─ SQLite Durable Object（默认 Free 后端）
        ├─ Artifacts（托管 Git 对象与 refs）
        ├─ 原生 Git 源站（兼容后端，持久盘）
        └─ R2 pack + Durable Object 清单（r2 环境）
              └─ 仅在回退或重量级操作时启动 Git 容器
```

### 3.1 各层职责

| 层 | 职责 | 不保存 |
|---|---|---|
| Rust Worker | 路由、会话、权限、Web 页面、Git 认证、协议代理、搜索与比较入口 | Git 对象正文（R2/DO/Artifacts 保存） |
| D1 | 用户、token 哈希、仓库元数据、权限、议题、PR、审查、通知、Webhook、任务状态 | Git pack 和 blob |
| 仓库 Durable Object | Git 对象的元数据与引用事务；R2 pack 清单和发布事务；推送串行化 | 账户级权限的唯一副本 |
| R2 `GIT_PACKS` | 不可变 pack 与 v2 index；接收中的分片对象 | refs 的权威状态 |
| R2 `LFS_OBJECTS` | Git LFS 对象，按仓库/分叉命名空间隔离 | Git 对象 |
| 容器 | 原生 Git 的回退执行：大推送、合并、比较、导入、pack compaction；CI 镜像构建 | 长期权威数据；工作区是临时盘 |

权限检查始终发生在 Worker。Git 客户端不会拿到 Artifacts 仓库 token、原生源站
共享密钥或 R2 凭据。

## 4. Git 传输与存储路径

### 4.1 默认 Free：SQLite Durable Object

每个仓库有一个按存储 ID 命名的 SQLite Durable Object。它用
`durable-git@0.0.8` 处理 Smart HTTP、接收 pack、保存对象和 refs。Worker 负责
认证与授权，并把 Git 请求流式转发给对象。默认单次 push 预算为 32 MiB。
Web 合并执行同仓库 fast-forward；其他合并通过 Git 完成。

### 4.2 Artifacts

`artifacts`、`managed` 和 `managed-staging` 环境使用 Artifacts 保存 Git。
Shard 用 Workers binding 读取对象和历史，用 Git HTTPS 做引用发现与受支持的
引用更新。Artifacts 的 divergent 或跨仓合并交给 Git 合并容器，或交给配置的
原生 Git 桥。仓库 token 由 Worker 换取，并限制在单次请求内。

### 4.3 原生 Git 源站（兼容）

`native` 环境把 Git 流量转发到自管的 `git http-backend` 和持久盘。Worker
先做仓库解析和权限检查，再代理 Smart HTTP。大 pack 可以直接发送到源站，
pre-receive 回调重新检查 token 和分支规则。该路径保留给已有部署，不是新实例的
推荐方向。

### 4.4 R2 Git：无 VPS 的大仓库路径

`r2` 环境把普通 Git 对象保存为 R2 中的不可变 pack 和 index：

1. Worker 解析 `info/refs`，从 Durable Object 清单返回 refs。
2. clone/fetch 由 Worker 读取 pack 与 index，生成 v1/v2 传输包，支持
   `thin-pack`、`ofs-delta` 和浅克隆。
3. 网页的提交、树、blob、文件和首父历史也直接从 pack index 解析。
4. 小型 push 由 Worker 直接接收：
   - 解析 receive 命令和 pack；
   - 校验 header、trailer、对象 SHA-1 和引用；
   - 对 thin pack 从已有 R2 对象补全 base；
   - 重建完整 pack 和 v2 index；
   - 通过 `prepare/publish` 事务写 pack 清单和 refs。
5. 超过 16 MiB、对象图过大、校验失败或异常请求回退到容器。
6. 大 push 还可以通过 `git-remote-shard` 分片直传 R2，Worker 在发布前校验
   完整请求、当前写权限、分支保护和预期 ref。
7. 后台 compaction 在至少 8 个 pack 时合并最多 16 个小 pack，输入预算
   256 MiB；替换对象保留 24 小时。

R2 直读和直接 receive 都由 `SHARD_R2_DIRECT_READS` 控制，默认开启。设为 `0`
会让网页读取、clone/fetch 和 push 回到容器路径。**直接成功的 push 不会启动
容器**；容器只在真正回退或执行合并、比较、导入、compaction 时按需启动。
`SHARD_GIT_R2_IDLE_MS` 控制空闲容器保留时间，默认 10 分钟。

### 4.5 Git LFS

所有后端共用 LFS Batch API。对象按仓库或分叉隔离，上传需要写 token 和仓库写
权限，发布前校验 SHA-256 与大小。R2 绑定存在时支持代理上传、预签名 PUT 和
分片续传。删除仓库后立即撤销访问，并异步清理对象。

## 5. 认证与安全模型

- 浏览器会话使用 HTTP-only cookie；会话和 token 秘密只保存哈希。
- 表单使用会话绑定 CSRF token 和同源检查；恢复页面只发送站点 origin 作为
  referrer。
- Git 使用 Shard 用户名 + access token（作为密码）。token 有读/写 scope、
  过期时间和撤销状态。推送要求写 scope 和仓库写权限。
- 登录失败按来源/账户组合限流；API 入口有请求限流。
- Webhook 使用签名 secret；重试保留事件 ID、投递 ID、正文和签名。
- 提交签名使用账户上传的 OpenPGP 公钥验证原始提交字节；邮箱验证状态单独显示。
- 私有仓库的页面、Git、搜索、下载、通知和 LFS 读取都重新检查当前权限；
  授权撤销在后续请求生效。
- 仓库删除或转移后，旧 URL 在权限允许时继续解析；分叉保留 refs 和对象。

## 6. 协作工作流

1. 用户创建仓库并选择可见性。
2. 通过 HTTPS 推送初始提交；Worker 校验 token、写权限和分支规则，
   存储层发布 refs。
3. 推送产生未完成推送记录；Webhook 投递和 PR 比较刷新在同一 D1 事务中登记。
4. PR 保存源/目标快照和共同祖先，评论固定在比较版本上。目标分支前进后，
   review 仍指向原快照。
5. 保护分支可要求审查批准和 CI 检查。合并队列按顺序推进目标 ref；Free
   后端执行 fast-forward，其他后端通过 Git 合并。
6. 发布由 Git 标签固定；CI 结果按提交显示；通知在权限过滤后进入收件箱。

## 7. 部署选择

| 环境 | Git 存储 | 适用场景 | 主要依赖 |
|---|---|---|---|
| 默认 `shard-free` | 每仓库 SQLite Durable Object | 新 forge、个人项目、低成本 | Workers Free、D1、Static Assets、Rate Limiting、Cron |
| `managed` / `managed-staging` | Artifacts | 已有 Artifacts、需要托管合并与复制 | Workers Paid、Artifacts、Containers、Workflows、R2 |
| `r2` | R2 pack + DO 清单 | 大仓库、无 VPS、Cloudflare 托管 Git | Workers Paid、Containers、DO、D1、两个私有 R2 桶 |
| `artifacts` | Artifacts | 保留原始 Artifacts 实例 | Workers Paid、R2 |
| `native`（兼容） | 自管 Git 源站 | 已有原生部署 | Cloudflare + HTTPS Git 源站 + 持久盘 |

部署前为每个实例准备独立的 D1、Git 存储和 R2 资源。R2 环境需要为
`GIT_PACKS` 和 `LFS_OBJECTS` 分别创建限定到桶的 Object Read & Write
凭据，并给 `git-incoming/`、`lfs-incoming/` 配置过期和未完成分片清理规则。

## 8. 容量与限制

| 资源 | 关键限制 | Shard 的处理 |
|---|---|---|
| Worker | 128 MB 内存；Paid CPU 最高 5 分钟 | Git 正文流式转发；大对象操作有明确预算 |
| D1 | Free 500 MB、Paid 10 GB；单行 2 MB；查询串行 | 只存元数据；分页和批处理 |
| SQLite DO | 单对象存储上限受计划约束 | 每仓库一个对象；删除和清理异步执行 |
| R2 | 单次上传约 4.995 GiB；分片最多 10,000；同键并发写有限 | pack 不可变；接收分片在临时前缀 |
| 标准 HTTPS push | 受域名请求体上限约束 | 小 push 直接处理；大 push 用分片 helper |
| 容器工作区 | 4 MiB 控制元数据预算；实例规格影响临时盘 | refs/清单有界；大 pack 仍受工作区限制 |

具体数字和升级判断见 [部署容量指南](deployment-capacity.md) 与
[README 的 R2 章节](../README.md#large-repositories-on-cloudflare-r2)。

## 9. 运维与恢复

- **备份**：D1、Durable Object 清单、`GIT_PACKS` 中被引用的对象，以及
  `LFS_OBJECTS` 必须成组备份。只备份 D1 不能恢复历史 diff 或 LFS。
- **恢复**：先恢复 D1 和清单，再恢复 pack/index 与 LFS 对象；按仓库校验 refs
  和对象可用性。替换后的 pack 保留 24 小时，可用于旧读者回滚。
- **清理**：仓库删除、token 过期、登录限流、邮件链接和未完成推送由分钟 cron
  或每日任务处理；清理依赖 cron 持续运行。
- **观测**：响应带 `X-Request-ID`；Workers Logs 记录结构化错误；请求限流按
  Cloudflare 位置近似执行。
- **发布**：`scripts/cd.mjs` 可先发布 staging，验证后再发布生产；D1 迁移不会
  随 Worker 回滚。发布串行执行。

## 10. 本地开发与验证

```sh
mise install
mise run setup
mise run dev        # http://localhost:8787
```

常用检查：

```sh
mise run check      # Rust/Worker 类型检查
mise run lint       # Clippy
mise run build      # build/index.js
mise run test       # 三种后端
npm test            # 使用已有 build
```

R2 Git 集成测试需要 Linux、WSL2 或 macOS 上的 FUSE Docker：

```sh
mise run build
SHARD_TEST_R2_CONTAINER=1 mise exec -- node --test tests/r2-git.test.mjs
```

## 11. 仓库结构

| 路径 | 内容 |
|---|---|
| `src/` | Rust Worker：路由、认证、页面、Git 入口、各后端适配 |
| `worker/` | Node 侧 Git 存储与 R2 直读/直接接收实现 |
| `templates/` | Askama 服务端模板 |
| `static/` | CSS、字体、图片和缓存头 |
| `migrations/` | D1 迁移 |
| `containers/` | Git 容器与合并桥镜像 |
| `scripts/` | 部署、CD、管理、发布说明、原生 Git 辅助 |
| `tests/` | Worker、三后端、R2、Git 与 LFS 集成测试 |
| `docs/` | 部署、容量与项目说明 |
| `PLAN.md` | 交付顺序、能力边界与后端设计 |
| `STATUS.md` | 已验证行为、剩余工作和风险 |
| `CHANGELOG.md` | 版本记录 |

## 12. 当前状态与路线

已验证：本地 Free、Artifacts、native 和 R2 的 push/clone/fetch、PR 比较、
权限撤销、LFS、备份恢复演练和 R2 直读。R2 小型 push（包括 thin pack）已在
Worker 内完成校验和发布，直接成功的 push 不再预热容器。

仍需在目标账号验证：

- 托管 Free 配额和真实生产推送；
- R2 分片 `ListParts`、FUSE 范围读取、容器重启与目标实例规格；
- 大 LFS 上传的整对象哈希、条件发布和分片并发；
- 真实邮件发送域与邮箱；
- 托管 Artifacts 的 divergent/跨仓合并；
- 生产 partial clone/fetch 和外部 Git 更新事件接入。

进一步把容器限定为“只用于构建”，需要继续完成：

1. 大 push 的 Worker 流式 ingest，减少容器回退；
2. 直接 merge / compare，避免 Web 合并与比较依赖容器；
3. CI/CD 容器与 Git 回退容器分离；
4. 持久搜索索引和更完整的 Git 协议能力。

## 13. 文档索引

- [README](../README.md)：功能、部署、Git 访问、LFS、发布与环境
- [Cloudflare Dashboard 中文配置手册](configuration.zh-CN.md)：面板逐项配置
- [部署容量指南](deployment-capacity.md)：容量、分片、并发与升级
- [STATUS.md](../STATUS.md)：验证记录、限制、技术风险
- [PLAN.md](../PLAN.md)：交付阶段、Cloudflare 能力边界、后端取舍
- [CHANGELOG.md](../CHANGELOG.md)：版本变更