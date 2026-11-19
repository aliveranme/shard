# Shard Cloudflare Dashboard 中文配置手册

以 Shard 稳定版 `v0.3.1` 为例。新建个人实例可以从 [Free 一键部署](#free-一键部署与自动更新) 开始。已有线上数据时，从 [接入已有仓库或 Worker](#接入已有仓库或-worker) 开始。自定义域名、R2、邮件和运行时密钥的配置也在本手册。使用命令行的读者可跳到 [本地预检与手动部署](#本地预检与手动部署)。[稳定版下载](https://github.com/aliveranme/shard/releases/latest) · [Wrangler 配置模板](../wrangler.jsonc)

## 选择部署方案

| 使用场景 | Wrangler 环境 | Git 存储与部署条件 |
| --- | --- | --- |
| 新建个人或小团队实例，想从私有仓库自动更新 | 默认 Free，省略 `--env` | D1 存业务数据，SQLite Durable Objects 存 Git。单次普通 Git push 默认上限 32 MiB。需要 LFS 时再绑定私有 R2 桶 |
| 给已经运行的 Artifacts 实例升级 | `artifacts` | 使用原 Worker、D1、Artifacts 命名空间、R2 桶和域名。普通 Git 单次 push 上限 64 MiB |
| 使用 Artifacts，需要 Web 合并桥和预发布验证 | `managed` 与 `managed-staging` | Workers Paid、Artifacts、Containers、Workflows 和各环境独立的 D1、R2 |
| 普通 Git pack 经常过大，希望继续由 Cloudflare 托管 | `r2` | Workers Paid、Containers、Workflows、D1 和两个私有 R2 桶。大 push 通过 `git-remote-shard` 分片续传 |
| 已有 VPS 和持久盘，希望标准 Git 直接传大 pack | `native` | Worker 托管 Web 与权限检查，Git 源站使用独立 HTTPS 域名和持久盘。标准 Git 直连源站 |

个人实例可以先用 Free，观察实际 push 的压缩 pack 大小。如果大对象是图片、模型或压缩包等可用 LFS 管理的文件，先评估在原实例上启用 LFS。LFS 配置会影响之后用 LFS 跟踪的文件，已有普通 Git 历史仍占原后端空间。需要扩大普通 Git 的 push 或仓库容量时，再比较 `r2` 和 `native`。[各后端容量与计费边界](deployment-capacity.md#数量容量与执行限制)

## Cloudflare Dashboard 部署与自动更新

### Free 一键部署与自动更新

1. 点击 Shard 的部署按钮，登录 Cloudflare，在账号选择器中切换到要运行 Shard 的账号。接下来会进入 **设置您的应用程序**（Set up your application）。[Deploy to Cloudflare](https://deploy.workers.cloudflare.com/?url=https%3A%2F%2Fgithub.com%2Faliveranme%2Fshard)
2. 逐项填写下表。第一次连接 GitHub 或 GitLab 时，按 Git 提供方的页面授权。选择有权创建仓库的个人账号或组织。[GitHub 授权和安装范围](https://developers.cloudflare.com/workers/ci-cd/builds/git-integration/github-integration/#install-and-authorize-the-app)

   | 面板字段 | 新建 Free 实例怎么填 |
   | --- | --- |
   | Git 帐户 / Git account | 选择 GitHub 或 GitLab 下要存放 Shard 源码的账号 |
   | 创建专用 Git 存储库 | 勾选。Cloudflare 会创建私有部署仓库并绑定 Worker。在 Shard 内新建的业务仓库有各自的可见性设置 |
   | 项目名称 / Project name | 默认 `shard-free`，也可填账号内独立的 Worker 名，例如 `shard-personal` |
   | Select D1 数据库 | 新建实例选 `+ 新建`，填 `shard-personal-db` 等独立库名。也可选择已为此实例准备好的空库。已有线上数据请按 [接入已有仓库或 Worker](#接入已有仓库或-worker) 保留原绑定 |
   | 数据库位置提示、读取复制 | 个人实例保留面板默认值。位置提示帮助 Cloudflare 选部署区域。有数据驻留要求时，预先创建指定辖区的 D1，再从列表选择该库 |
   | 运行时变量 | 保留面板预填的 `GIT_BACKEND=durable`、`SHARD_LIMIT_PROFILE=free`、`MAX_PUSH_MB=32`、`SHA1DC=1` |
   | 构建命令 / Build command | `npm run build` |
   | 部署命令 / Deploy command | `npm run deploy`，这一步会先运行 D1 迁移 |
   | 预览命令 / Preview command | 保留 `npx wrangler preview`。只想更新生产分支时，可关闭「启用预览构建」 |
   | 使用 Cloudflare Access 保护 | 希望用普通 Git HTTPS 访问 Shard 时保持关闭。私库由 Shard 的仓库权限和访问令牌限制 |
   | 高级设置 → 根目录 / Root directory | `/`，即包含 `package.json` 和 `wrangler.jsonc` 的仓库根目录 |
   | 高级设置 → API 令牌 | 选择构建令牌或创建新令牌。按下节核对它是否有执行 D1 迁移的权限 |

3. 点击 **部署**。在 **Workers 和 Pages → 目标 Worker → 部署（Deployments）→ 查看构建历史（View build history）** 打开首个构建。构建和部署成功后，从 Worker 的概览打开分配的 `workers.dev` 域名。[Workers Builds 设置](https://developers.cloudflare.com/workers/ci-cd/builds/configuration/)
4. 打开 **设置 → 构建（Build）→ 分支控制（Branch control）**，确认关联的是刚创建的私有仓库，生产分支是你要发布的分支，通常为 `main`。在 Worker 的 **绑定（Bindings）** 页检查 `DB` 指向所选数据库，并核对 `GIT_REPOS` 和 `ASSETS`。D1 的 **表（Tables）** 应能看到 `users` 和 `repos`。[生产分支](https://developers.cloudflare.com/workers/ci-cd/builds/build-branches/#change-production-branch)

### 选择构建 API 令牌

1. 从右上角用户菜单打开 **我的个人资料（My Profile）→ API 令牌（API Tokens）**，选择 **创建令牌（Create Token）**，使用自定义令牌。将账号资源范围限制到部署 Shard 的账号。已有构建令牌可在这一页编辑权限。[创建用户 API 令牌](https://developers.cloudflare.com/fundamentals/api/get-started/create-token/)

   | 权限范围 | 权限 | 用途 |
   | --- | --- | --- |
   | Account | Workers Scripts Edit、D1 Edit | 发布 Worker、运行 D1 迁移 |
   | Account | Account Settings Read | 构建令牌的账号识别权限 |
   | Account | Workers R2 Storage Edit | 部署需要 R2 资源的环境时使用 |
   | Zone | Workers Routes Edit | 部署包含域名路由的配置时，范围选对应 zone |

2. 返回一键部署的 **高级设置 → API 令牌**，或进入已有 Worker 的 **设置 → 构建**，选择该令牌。若由面板自动创建令牌，以面板显示的令牌名称在用户 API 令牌列表中定位它，补齐 **D1 Edit**。若第一次构建因迁移权限失败，在构建记录里用 **重试构建（Retry build）** 重新执行。[Workers Builds 的默认令牌权限](https://developers.cloudflare.com/workers/ci-cd/builds/configuration/#api-token)
3. 构建令牌用于管理账号资源。R2 的 Access Key ID 和 Secret Access Key 在 R2 管理页面生成，随后作为对应 Worker 的运行时 Secret 添加。

### 接入已有仓库或 Worker

1. 先在 GitHub 或 GitLab 准备含 Shard 源码的私有仓库。准备部署固定版本时，用对应标签的源码初始化所选生产分支。已有 Worker 时，先打开它的 **绑定** 和 **设置 → 域和路由**，记录原 D1 UUID、Git 存储标识、R2 桶和域名，然后将这些值填进部署仓库的 `wrangler.jsonc`。Free 改顶层，命名环境改 `env.<环境>`。
   对需要运行时密钥的环境，按 [各环境的配置表](#各部署环境怎么配) 先在目标 Worker 保存变量和密钥。新实例可在 **Workers 和 Pages → 创建应用 → Start with Hello World!** 先创建与 `name` 同名的 Worker，保存密钥后再连接仓库。已有实例直接使用原 Worker。
2. 新建 Worker 时，在 **Workers 和 Pages → 创建应用（Create application）** 选择 GitHub 或 GitLab，然后选择已有仓库。已有 Worker 时，从 **该 Worker → 设置 → 构建 → 连接（Connect）** 选仓库。GitHub 私库未出现时，在 **GitHub → Settings → Applications → Installed GitHub Apps → Cloudflare Workers and Pages → Configure** 检查该仓库是否在授权范围。[连接入口](https://developers.cloudflare.com/workers/ci-cd/builds/#connect-an-existing-worker) · [GitHub 仓库权限](https://developers.cloudflare.com/workers/ci-cd/builds/git-integration/github-integration/#repository-access)
3. **根目录**填 `/`，**生产分支**选 `main` 或实际用于发布的分支，**API 令牌**选上节设置的令牌。按下表设置命令。所选环境在 Wrangler 中的 `name` 与连接的 Worker 名一致。

   | 目标 Worker | 构建命令 | 部署命令 |
   | --- | --- | --- |
   | 默认 Free | `npm run build` | `npm run deploy` |
   | 已有 Artifacts | `npm run build` | `npm run deploy -- --env artifacts` |
   | 自管原生 Git | `npm run build` | `npm run deploy -- --env native` |
   | 直接发布托管环境 | `npm run build` | `npm run deploy -- --env managed` 或 `npm run deploy -- --env r2` |
   | 托管 Paid，先验证 staging | `npm run ci -- --container` | `npm run cd`，连接生产 Worker |

4. 保存后向关联的生产分支推送一个提交。在新建应用流程也可用 **部署** 启动首个构建。要测试非生产分支，单独启用预览构建，为预览准备独立的 D1 和 Git 存储。[预览配置](https://developers.cloudflare.com/workers/previews/configuration/)

### 发布下一次更新

1. 在自己的私有部署仓库修改代码或配置。升级 Shard 时，先从目标版本取来改动，放到部署仓库的更新分支，检查 `wrangler.jsonc` 中的 Worker 名、D1 UUID 和其他资源标识，然后合入关联的生产分支。Cloudflare 以这个私有仓库的生产分支为发布触发源。
2. 在 **Worker → 部署 → 查看构建历史** 找到该提交。查看配置的构建命令、D1 迁移和发布结果，在 **部署** 页确认新版本已激活。使用 Free 部署时，核对版本标签与提交 SHA。
3. 打开原 Worker 域名，确认旧数据可见。新实例可注册测试账户、创建私库，用写令牌 push 后重新 clone。检查另一账号在缺少权限时无法 clone。需要 LFS 时再验证对象上传与下载。[首次推送示例](../README.md#first-repository)

### 在 Dashboard 创建并核对资源绑定

1. **D1**。打开 **存储和数据库 → D1 SQL 数据库 → 创建数据库（Create Database）**。填库名并创建，从数据库详情复制 ID。更新部署仓库中 `d1_databases` 的 `database_name`、`database_id`，使用 `binding = "DB"` 和 `migrations_dir = "migrations"`。已有 Worker 如缺少 `DB`，也可打开 **绑定 → 添加绑定 → D1 database**，变量名填 `DB` 并选择该库。部署后回到 **绑定** 核对库名，并从 D1 的 **表（Tables）** 检查 `d1_migrations`、`users` 和 `repos`。[D1 Dashboard 操作](https://developers.cloudflare.com/d1/get-started/#3-bind-your-worker-to-your-d1-database)
2. **R2**。开通 R2 后，打开 **R2 对象存储 → 创建存储桶**，为 LFS 和 R2 Git 按需建立独立私有桶。把桶名加入所选环境的 `r2_buckets`，变量名分别为 `LFS_OBJECTS` 和 `GIT_PACKS`。在桶详情复制 **S3 API** endpoint，辖区桶使用对应辖区 endpoint。[R2 存储桶](https://developers.cloudflare.com/r2/buckets/create-buckets/) · [R2 涉及的计费](https://developers.cloudflare.com/r2/pricing/)
3. **R2 分片与签名凭据**。在 **R2 → 概览 → Account Details → API Tokens → Manage** 创建 R2 令牌。选择 **Object Read & Write** 并限定到目标桶，保存显示的 **Access Key ID** 和 **Secret Access Key**。在 **桶 → Settings → Object Lifecycle Rules → Add rule** 按前缀 `lfs-incoming/` 或 `git-incoming/` 设置 7 天到期和中止未完成分片。每个前缀配置在对应桶，正式对象保存在 `lfs/` 和 `git-packs/`。[桶级凭据](https://developers.cloudflare.com/r2/api/tokens/) · [生命周期规则](https://developers.cloudflare.com/r2/buckets/object-lifecycles/#configure-lifecycle-rules-for-your-bucket)
4. **Artifacts 和 Containers**。在已开通 Workers Paid 的账号中，为 `artifacts` 或 `managed` 填 `ARTIFACTS` 绑定的命名空间。已有实例填写原命名空间。`managed`、`managed-staging` 和 `r2` 保留所选环境的 `containers`、对象类迁移和 Workflow 定义，构建时由 Wrangler 发布。部署后在 **Workers 和 Pages → Containers**、**Workflows** 查看相应应用或工作流，验证一次会触发容器的 Git 操作。[Artifacts 命名空间](https://developers.cloudflare.com/artifacts/concepts/namespaces/) · [Containers 发布](https://developers.cloudflare.com/containers/guides/deploy/#deploy-with-workers-builds)

### 在 Dashboard 添加运行时变量与密钥

在 **Workers 和 Pages → 目标 Worker → 设置 → 变量和密钥（Variables and Secrets）** 点击 **添加（Add）**。文本值选 **Text**，凭据选 **Secret**。填写名称和值后点击 **部署（Deploy）**。`managed-staging` 的变量和密钥填写在 staging Worker，生产填写在生产 Worker。更改两个环境时分别发布。各环境所需的键见 [配置各部署环境](#各部署环境怎么配)。[Dashboard 添加 Worker Secret](https://developers.cloudflare.com/workers/configuration/secrets/#via-the-dashboard)

### 域名与定时触发器

需要分配域名时，在 **目标 Worker → 设置 → 域和路由（Domains & Routes）→ 添加 → 自定义域（Custom Domain）** 填入账号内的域名，例如 `forge.example.com`。若随 Git 管理域名，把该主机名填入所选配置的 `routes`。保存后在浏览器打开 HTTPS 地址，配置了邮件的实例同时把 `PUBLIC_ORIGIN` 更新为该主机名的 HTTPS origin。[自定义域名](https://developers.cloudflare.com/workers/configuration/routing/custom-domains/#add-a-custom-domain)

在 **目标 Worker → 设置 → 触发器（Triggers）→ Cron Triggers** 核对两条计划。`* * * * *` 每分钟处理队列，`0 3 * * *` 每日 03:00 UTC 清理过期记录。让部署仓库的 `triggers.crons` 与面板保持一致。[Cron Triggers](https://developers.cloudflare.com/workers/configuration/cron-triggers/)

## 配置放在哪里

| 位置 | 作用 |
| --- | --- |
| Wrangler 顶层或 `env.<名称>` | 源码使用 `wrangler.jsonc`，预构建包使用 `wrangler.json`。这里声明 Worker 名、D1、R2、DO、Artifacts、路由、Cron 和运行时文本 `vars`。Free 使用顶层。命名环境的绑定和文本变量写在所选环境内 |
| Worker「设置 → 变量和密钥」 | 线上运行时的 Text 或 Secret。配置 `managed-staging` 时进入 staging Worker，生产同理 |
| Workers Builds「构建变量和密钥」 | 构建进程的变量和凭据，如 `SHARD_RELEASE_BRANCH`。部署权限由构建 API 令牌提供 |
| 本地部署 shell | `CLOUDFLARE_ACCOUNT_ID` 选择账号。交互式使用 Wrangler 登录，非交互式从环境提供 `CLOUDFLARE_API_TOKEN`。`CLOUDFLARE_ENV` 可设定默认的命名环境，显式 `--env` 优先 |
| `.env.deploy.*` 或私有 JSON 文件 | 通过 `--secrets-file` 注入线上 Worker Secret。项目忽略 `.env.deploy.*` 文件。密钥文件内填写实际值 |
| `.dev.vars` 或 `.dev.vars.<环境>` | Wrangler 本地开发用的密钥。各命名环境可使用自己的文件，例如 `.dev.vars.native` |

模板的 `keep_vars: true` 会保留控制台管理的文本变量。Wrangler 配置中声明的同名 `vars` 每次部署都会上传。要改为在控制台维护某个文本值，先在对应 Worker 设置该值，再从所选环境的 `vars` 移除同名项。已有 Worker Secret 会保留，首次部署或轮换时通过控制台或 `--secrets-file` 提供。API 令牌、R2 凭据和共享密钥放在密钥中。[变量优先级与配置来源](https://developers.cloudflare.com/workers/wrangler/configuration/#source-of-truth) · [Worker Secret](https://developers.cloudflare.com/workers/configuration/secrets/)

### 通用资源与变量

| 项目 | 配置方式 |
| --- | --- |
| `name` 和 `account_id` | Worker 名与 Cloudflare 面板中的目标一致。账号可在部署环境中选定。配置中显式填写 `account_id` 时，需与登录或构建所选账号一致 |
| `DB` | `d1_databases` 中 `binding` 为 `DB`，`migrations_dir` 为 `migrations`。已有实例填写原 `database_id`。新建时可由面板分配，命令行也可用默认占位 ID 或省略 ID，由脚本按 `database_name` 查找或创建 |
| `ASSETS` | 使用模板的 `assets.directory = "./static"` 与 `binding = "ASSETS"` |
| `REQUEST_LIMITER` | 保留模板中的 `ratelimits` 绑定，默认每个限流键 60 秒 300 次。新实例使用各自的 `namespace_id` |
| Cron | 保留每分钟的 `* * * * *` 和每日 03:00 UTC 的 `0 3 * * *`，用于处理待办工作和清理过期记录 |
| `GIT_BACKEND` | `durable`、`artifacts`、`r2` 或 `native`。按部署模板保留对应值。它选择新仓库的后端，已有仓库继续使用 D1 记录的后端，fork 沿用源仓库后端 |
| `SHARD_LIMIT_PROFILE` | `free` 或 `paid`。它控制 Shard 的搜索、后台任务和上传预算。Workers 套餐须在 Cloudflare 账号中管理 |
| `SHARD_REGISTRATION` | `open` 或 `closed`，默认 `open`。设为 `closed` 后注册页和访客入口隐藏注册，提交注册返回 403；已有账号仍可登录 |
| `MAX_PUSH_MB`、`INGEST_CACHE_MB`、`SHA1DC` | SQLite DO 后端的 push 与解析设置。`MAX_PUSH_MB` 可设 1–32 MiB，默认 32 MiB。`INGEST_CACHE_MB` 可设 1–16 MiB，默认 16 MiB。保留 `SHA1DC = "1"` 以启用 SHA-1 碰撞检测 |

按名称自动复用 D1 时，脚本只接受空库或能识别为 Shard 的数据库。已有实例使用显式 UUID 可以让升级始终指向原库。升级迁移追加执行，保留已经执行的迁移文件和 Durable Object 迁移标签。[D1 迁移](https://developers.cloudflare.com/d1/reference/migrations/) · [现有模板](../wrangler.jsonc)

取用 Cloudflare 自定义域时，在所选环境的 `routes` 可写 `[{ "pattern": "forge.example.com", "custom_domain": true }]`，其中 `pattern` 是实际主机名。发布前核对 **域和路由** 显示的生产域名。[Wrangler 路由配置](https://developers.cloudflare.com/workers/configuration/routing/custom-domains/#set-up-a-custom-domain-in-your-wrangler-configuration-file)

## 本地预检与手动部署

源码方式使用 Git 2.32 以上和 mise。从 checkout 执行以下命令即可启动本地 D1 与 SQLite Durable Objects，浏览器访问 `http://localhost:8787`。

```sh
mise install
mise run setup
mise run dev
```

部署固定版本可下载 `shard-v0.3.1-worker.tar.gz` 和同名 `.sha256`。Linux 用 `sha256sum --check shard-v0.3.1-worker.tar.gz.sha256`，macOS 用 `shasum -a 256 -c shard-v0.3.1-worker.tar.gz.sha256`。Windows PowerShell 可执行 `Get-FileHash -Algorithm SHA256 .\shard-v0.3.1-worker.tar.gz`，与校验文件中的摘要比对。核对后运行 `tar -xzf shard-v0.3.1-worker.tar.gz`，进入 `shard-v0.3.1-worker` 目录，使用包内 `.node-version` 指定的 Node 版本执行 `npm ci`。预构建包包含 Worker Wasm 与静态资源，带 Containers 的环境仍需 Docker 引擎构建镜像。[Release 产物](https://github.com/aliveranme/shard/releases/tag/v0.3.1) · [打包内容](../README.md#project-releases)

以下命令在项目或解包目录执行。源码目录用 `mise exec --` 运行命令以选用固定工具链。发布前用 `whoami` 核对账号。`--dry-run` 只预打包和验证绑定，正式部署才会执行线上 D1 迁移与发布。

```sh
npx --no-install wrangler login
npx --no-install wrangler whoami
npm run deploy -- --dry-run
npm run deploy
```

命名环境在预检和部署时都传同一个 `--env`。例如 `npm run deploy -- --env artifacts --dry-run`。`--config PATH` 可指定另一份配置。`deploy:paid` 的 dry-run 可省略密钥文件，正式发布前设置 `CLOUDFLARE_ACCOUNT_ID`，例如把真实账号 ID 写入 shell 的 `export CLOUDFLARE_ACCOUNT_ID=你的账号ID`，PowerShell 使用 `$env:CLOUDFLARE_ACCOUNT_ID = '你的账号ID'`。普通 `npm run deploy` 可复用已经设置好的运行时 Secret。[Wrangler 认证](https://developers.cloudflare.com/workers/wrangler/commands/general/#login)

## 各部署环境怎么配

### 默认 Free

在仓库 `wrangler.jsonc` 的顶层填写 Worker 名和 D1 数据库。下面是新建 `shard-personal` 的配置片段，把示例 UUID 替换为 Dashboard 上复制的数据库 ID，合并进现有配置文件。

```jsonc
{
  "name": "shard-personal",
  "d1_databases": [{
    "binding": "DB",
    "database_name": "shard-personal-db",
    "database_id": "11111111-2222-4333-8444-555555555555",
    "migrations_dir": "migrations"
  }]
}
```

Free 模板使用 `GIT_BACKEND = "durable"`、`SHARD_LIMIT_PROFILE = "free"`、`GIT_REPOS` → `GitRepository` 和 `free-git-v1` SQLite 类迁移。部署命令填 `npm run deploy`。需要 LFS 时，在顶层 `r2_buckets` 添加 `{ "binding": "LFS_OBJECTS", "bucket_name": "shard-personal-lfs" }`，替换为自己的私有桶名。[默认模板](../wrangler.jsonc)

### 已有 Artifacts 实例升级

从现有 Worker 记录原 Worker 名、D1 UUID、`ARTIFACTS` 命名空间、私有 `LFS_OBJECTS` 桶、路由和已启用的可选绑定。在 `env.artifacts` 填写这些原值并保留 `GIT_BACKEND = "artifacts"`。先按升级步骤备份，再运行以下命令。使用 Workers Builds 时可把第二行设为部署命令。

```sh
npm run deploy -- --env artifacts --dry-run
npm run deploy -- --env artifacts
```

此环境的 LFS 默认走 `proxy`。需要分叉历史或跨仓库的 Web 合并时，可用 Git 2.38 以上的外部合并桥。在 Worker 配置 `NATIVE_GIT_ORIGIN` 和 Secret `NATIVE_GIT_TOKEN`。源站使用同一个 `SHARD_GIT_TOKEN`，设置 `SHARD_GIT_ARTIFACTS_ORIGINS` 为对应账号的 Artifacts HTTPS origin。专用合并桥可运行在 `SHARD_GIT_BRIDGE_ONLY=1` 模式。[原 Artifacts 实例升级](../README.md#upgrade-an-existing-artifacts-deployment)

### 托管 Paid 与 staging

账号需开通 Workers Paid、Artifacts 和 Containers。为 `env.managed`、`env.managed-staging` 各自配置 Worker 名、D1、`ARTIFACTS`、`LFS_OBJECTS`、路由及两条独立的 Workflow 名称，保留模板中的 `MERGE_COORDINATOR`、`ARTIFACTS_MERGE_CONTAINER`、`REPOSITORY_COPY_WORKFLOW`、`RELEASE_VERIFICATION_WORKFLOW` 和 `SHARD_VERSION`。`SHARD_GIT_MERGE_INSTANCE` 可调整合并容器规格，默认 `standard-1`。

用 Dashboard 配置时，在生产和 staging 各自的 **变量和密钥** 页添加 Text `SHARD_GIT_ARTIFACTS_ORIGINS`，值取账号的 Artifacts Git HTTPS origin，例如 `https://<ACCOUNT_ID>.artifacts.cloudflare.net`。使用分片 LFS 时，两个 Worker 分别添加下方两个 LFS Secret，值取对应环境的私有桶凭据，再按 [LFS 配置](#git-lfs-的配置与使用) 添加 S3 endpoint、桶名和上传模式。

使用命令行 `deploy:paid` 时，每个环境的密钥文件包含这两个键。脚本会配置 R2 S3 endpoint、桶名和账号的 Artifacts HTTPS origin，并把 `SHARD_LFS_UPLOAD_MODE` 默认为 `r2-multipart`。若所选环境的 `vars` 已显式设置该变量，脚本保留显式值。

```dotenv
SHARD_LFS_R2_ACCESS_KEY_ID=替换为目标桶的访问密钥ID
SHARD_LFS_R2_SECRET_ACCESS_KEY=替换为目标桶的访问密钥
```

```sh
npm run deploy:paid -- --env managed-staging --secrets-file .env.deploy.staging
npm run deploy:paid -- --env managed --secrets-file .env.deploy.paid
```

两阶段自动部署使用源码仓库。在**生产 Worker**的 **设置 → 构建** 连接部署仓库，设置构建命令 `npm run ci -- --container` 和部署命令 `npm run cd`。这个命令会部署 staging 和生产两个 Worker。非 `main` 生产分支需要在 **设置 → 构建 → 构建变量和密钥** 新增 Text `SHARD_RELEASE_BRANCH`，值与生产分支一致。构建令牌需覆盖同账号的两套环境和 R2 资源。在有 Docker 的源码目录可先执行 `npm run cd -- --dry-run`，检查两套资源的绑定和镜像。

CD 先发布 staging，待 Workflow 验证版本、D1、R2、静态资源和合并桥后，再发布并验证同一提交的生产版本。若生产发布失败、活动部署仍属于本次执行且此前只有一个活动版本，CD 会尝试恢复该版本。运行中的容器可能仍在用启动时的镜像，已应用的 D1 迁移会保留。让同一生产 Worker 的发布串行执行。[托管发布机制](../README.md#deploy-with-managed-cloudflare-services)

### Cloudflare 托管 R2 Git

在 `env.r2` 配置 `DB` 和两个私有桶。`GIT_PACKS` 保存普通 Git pack 与索引，`LFS_OBJECTS` 保存 LFS 对象。保留 `GIT_R2_REPOS`、`MERGE_COORDINATOR`、`r2-git-v1`、复制 Workflow 和 `containers/git-r2.Dockerfile`。为两个桶分别生成限定到该桶的 **Object Read & Write** 凭据，配置 Git 与 LFS 的暂存生命周期。[R2 桶级凭据](https://developers.cloudflare.com/r2/api/tokens/)

网页读取直接从 R2 的 pack 与 index 解析提交、树、文件内容和首父历史；clone 与 fetch 也由 Worker 从这些 pack 生成传输包。容器只处理 push、合并、比较、导入和 pack compaction。设置 `SHARD_R2_DIRECT_READS=0` 可强制读取回退到容器。Durable Object 会保存容器会话令牌，`SHARD_GIT_R2_IDLE_MS` 控制空闲容器保留时间，默认 10 分钟。

Shard 的部署目标是完全在 Cloudflare 上托管 Git。`native` 源站保留为已有部署的兼容后端，但不是项目推荐方向，也不用于解决容器冷启动。

`.env.deploy.r2` 写入上一节的两个 LFS 密钥，再添加下列两个键。设置目标账号 `CLOUDFLARE_ACCOUNT_ID`，在有 Docker 的环境使用 Paid 预设发布。

```dotenv
SHARD_GIT_R2_ACCESS_KEY_ID=替换为Git桶的访问密钥ID
SHARD_GIT_R2_SECRET_ACCESS_KEY=替换为Git桶的访问密钥
```

```sh
npm run deploy:paid -- --env r2 --dry-run
npm run deploy:paid -- --env r2 --secrets-file .env.deploy.r2
```

| 运行时配置 | 说明 |
| --- | --- |
| `SHARD_GIT_R2_BUCKET`、`SHARD_GIT_R2_ENDPOINT` | 与 `GIT_PACKS` 对应的桶名和 S3 HTTPS origin。Paid 预设会根据账号、桶名和辖区生成 |
| `SHARD_GIT_R2_MAX_PUSH_BYTES` | Git receive-pack 请求上限，默认 2 GiB。按容器工作盘和验证耗时调整。标准 HTTPS 仍受域名请求体限制 |
| `SHARD_GIT_R2_INSTANCE` | Git 容器规格，默认 `standard-4`。临时盘用于收包、验证和合并 |
| `SHARD_R2_DIRECT_READS` | 默认开启。设为 `0` 时网页读取和 clone/fetch 回退到 Git 容器 |
| `SHARD_GIT_R2_IDLE_MS` | 空闲容器保留时间，默认 `600000`（10 分钟），最低 1 分钟 |
| `SHARD_GIT_IMPORT_HOSTS` | 可选，配置 Git 容器可导入的 HTTPS 主机名，用逗号分隔 |

在 `GIT_PACKS` 为 `git-incoming/` 设置七天过期和中止未完成分片上传的规则，保留存放正式数据的 `git-packs/`。在本地源码或 Release 目录安装 `npm install -g .`，再到要推送的 checkout 设置单独的 push URL，替换域名、所有者和仓库名。

```sh
git remote set-url --push origin 'shard::https://forge.example.com/owner/repo.git'
git push origin main
```

普通 HTTPS 继续用于 clone 和 fetch。中断时，在原 checkout 运行 `git remote-shard --list https://forge.example.com/owner/repo.git` 找到上传 ID，再以 `git remote-shard --resume 上传ID https://forge.example.com/owner/repo.git` 续传。有 LFS 时额外设置 `git config --local remote.origin.lfspushurl 'https://forge.example.com/owner/repo.git/info/lfs'`，并按下节配置 LFS 上传适配器。[续传、保留期与 R2 Git 验证](deployment-capacity.md#r2-git全托管大仓)

用 Workers Builds 发布 `r2` 时，在目标 Worker 的 **变量和密钥** 中添加表中的 Git 桶变量和两个 Git 桶 Secret，另加 [LFS 配置](#git-lfs-的配置与使用) 中的文本变量与 Secret。构建令牌需有 **Workers Scripts Edit、D1 Edit 和 Workers R2 Storage Edit**。构建命令填 `npm run build`，部署命令填 `npm run deploy -- --env r2`。在账号的 **Containers** 查看镜像和实例状态，再用一个小仓库做真实 push，确认 Git 容器能接收并发布 pack。[带 Containers 的 Git 构建](https://developers.cloudflare.com/containers/guides/deploy/#deploy-with-workers-builds)

### 自管原生 Git 源站

在 VPS 安装项目指定的 Node 和 Git 2.32 以上的版本。需要 Web 非快进合并时使用 Git 2.38 以上。用绝对路径配置 `SHARD_GIT_DIRECTORY` 指向持久卷，给 `SHARD_GIT_TOKEN` 生成独立随机密钥。一个持久卷运行一个源站实例。下方示例配置直连模式。

| 位置与变量 | 配置 |
| --- | --- |
| 源站 `SHARD_GIT_HOST`、`SHARD_GIT_PORT` | 默认 `127.0.0.1` 和 `8793`，用 HTTPS 反代流式转发请求与响应 |
| 源站 `SHARD_GIT_IMPORT_HOSTS` | 可选，替换允许导入的主机，以逗号分隔。默认 `github.com,gitlab.com,codeberg.org` |
| Worker `NATIVE_GIT_ORIGIN` | 在 `env.native.vars` 填源站 HTTPS origin，例如 `https://git.example.com`，保留主机和可选端口 |
| Worker `NATIVE_GIT_TOKEN` | 在 `native` Worker 的 **变量和密钥** 页添加 Secret，值与源站的 `SHARD_GIT_TOKEN` 一致 |
| `LFS_OBJECTS` | 模板内已有 R2 绑定。有 LFS 需求时填写实际私有桶名。只使用普通 Git 可以移除 `env.native` 中的该绑定 |
| Worker `NATIVE_GIT_DIRECT_ORIGIN`、源站 `SHARD_GIT_FORGE_ORIGIN` | 需要大 pack 直连时，前者填写 Git 源站 HTTPS origin，后者填写 Shard Web Worker 的 HTTPS origin。Git 用页面给出的新克隆地址 |
| 源站 `SHARD_GIT_TRUST_PROXY` | 仅在源站监听环回接口且受信反代覆盖客户端 `X-Real-IP` 时设为 `1` |

VPS 私有文件 `.env.git-origin` 可按下方填写，替换可用的持久盘路径、共享密钥和实际域名。用进程管理器运行 `node --env-file=.env.git-origin scripts/git-origin.mjs`。

```dotenv
SHARD_GIT_DIRECTORY=/srv/shard/git
SHARD_GIT_TOKEN=替换为源站与Worker共享的随机密钥
SHARD_GIT_FORGE_ORIGIN=https://forge.example.com
```

在 `wrangler.jsonc` 的 `env.native.vars` 中更新源站设置。下面展示这两个字段在 `env.native` 中的位置。

```jsonc
{
  "env": {
    "native": {
      "vars": {
        "NATIVE_GIT_ORIGIN": "https://git.example.com",
        "NATIVE_GIT_DIRECT_ORIGIN": "https://git.example.com"
      }
    }
  }
}
```

在 Cloudflare 打开对应域名的 **DNS → 记录（Records）**，用源站的地址新增 `git.example.com` 的 A、AAAA 或 CNAME，并设 **DNS only**。在源站反代设置 TLS、流式转发、请求大小与读写超时。Nginx 需要关闭请求及响应代理缓冲。Worker 的 Web 域名 `forge.example.com` 通过 **域和路由** 添加。普通 Git 源站模式只填 `NATIVE_GIT_ORIGIN` 并使用 Worker 提供的 Git HTTPS URL。[HTTPS 反代配置示例](../README.md#large-repositories-on-workers-paid) · [DNS 代理状态](https://developers.cloudflare.com/dns/proxy-status/)

使用 Dashboard 自动更新时，在 `native` Worker 的构建里填写 `npm run deploy -- --env native`，并使用控制台中保存的 `NATIVE_GIT_TOKEN`。首次从命令行发布时，把同一 Secret 写入 `.env.deploy.native`，然后执行下列命令。

```sh
npm run deploy -- --env native --secrets-file .env.deploy.native
```

如需 Workers Paid 预算和 LFS 分片，准备同一个 `NATIVE_GIT_TOKEN` 和两个 LFS 桶级密钥，运行 `npm run deploy:paid -- --env native --secrets-file .env.deploy.paid`。Worker 发布与 VPS 更新分别执行。已有 Artifacts 仓库在原生实例中继续访问时，保留其 `ARTIFACTS` 绑定与原数据。

## Git LFS 的配置与使用

在所选环境把私有 R2 桶绑定为 `LFS_OBJECTS`，在客户端安装 Git LFS。使用 `git lfs track '*.zip'` 等匹配规则，将 `.gitattributes` 和需要跟踪的文件提交后推送。

| `SHARD_LFS_UPLOAD_MODE` | 适用情况 | 客户端 |
| --- | --- | --- |
| `proxy`（默认） | 默认 100,000,000 字节，经 Worker 转发，受域名请求体上限约束 | 标准 Git LFS |
| `r2-direct` | 客户端直接单次 PUT 到 R2。单对象上限 5,363,466,240 字节 | 标准 Git LFS |
| `r2-multipart` | 分片直传 R2，适合大文件或要恢复中断的上传。Paid 预设默认 5,363,466,240 字节 | Git LFS 加 Shard 上传适配器 |

使用直传或分片时，在目标 Worker 的 **设置 → 变量和密钥** 按下表添加配置。使用默认 `proxy` 时，配置 `LFS_OBJECTS` 绑定即可。

| 名称 | 类型 | 值 |
| --- | --- | --- |
| `SHARD_LFS_UPLOAD_MODE` | Text | `r2-direct` 或 `r2-multipart` |
| `SHARD_LFS_R2_BUCKET` | Text | `LFS_OBJECTS` 绑定中的 `bucket_name` |
| `SHARD_LFS_R2_ENDPOINT` | Text | 该桶的 S3 HTTPS origin，例如 `https://<ACCOUNT_ID>.r2.cloudflarestorage.com` |
| `SHARD_LFS_R2_ACCESS_KEY_ID` | Secret | 该桶的 Object Read & Write 凭据中的 Access Key ID |
| `SHARD_LFS_R2_SECRET_ACCESS_KEY` | Secret | 同一凭据中的 Secret Access Key |
| `SHARD_LFS_MAX_BYTES` | Text，可选 | 新上传的对象上限，填整数，单位字节 |

在桶的生命周期规则中为 `lfs-incoming/` 设置七天过期和中止未完成分片，保留正式对象前缀 `lfs/`。超过默认单 PUT 上限的 Paid 分片配置应按实际校验、发布和 fork 耗时验证。[LFS 容量表](deployment-capacity.md#lfs-对象上限) · [R2 生命周期规则](https://developers.cloudflare.com/r2/buckets/object-lifecycles/)

客户端使用分片模式时，在每个需要上传的 checkout 运行下列命令，将适配器路径替换为源码或 Release 包中的绝对路径。下载仍按标准 Git LFS 工作。

```sh
git config --local lfs.customtransfer.shard-r2-multipart.path node
git config --local lfs.customtransfer.shard-r2-multipart.args '"/absolute/path/to/scripts/git-lfs-r2-multipart.mjs"'
git config --local lfs.customtransfer.shard-r2-multipart.direction upload
```

## 邮件、Webhook 和访问令牌

- **注册与管理员**。注册默认开放。把 Text 变量 `SHARD_REGISTRATION` 设为 `closed` 后，注册页和访客导航不再显示注册入口，`POST /user/sign_up` 返回 403，已有账号仍可登录。管理员通过 D1 设置，不依赖环境变量：先用 `npm run admin -- list --env r2` 查看，再用 `npm run admin -- grant <用户名> --env r2` 授予；`--replace` 会先撤销其他管理员，`revoke` 撤销。使用其他配置文件时加 `--config`，本地库加 `--local`。管理员可越过仓库管理权限检查，只应授予可信账号。
- **验证邮箱与重置密码**。打开账号的 **Email Service → Email Sending → Onboard Domain**，选择使用 Cloudflare DNS 的发信域名，完成页面给出的 DNS 配置。回到 Worker 的 **变量和密钥** 添加 Text `PUBLIC_ORIGIN`（例如 `https://forge.example.com`）和 `EMAIL_FROM`（例如 `shard@example.com`）。将 `send_email: [{ "name": "EMAIL", "allowed_sender_addresses": ["shard@example.com"] }]` 加入所选 Wrangler 环境，使用与你实际 `EMAIL_FROM` 一致的地址。生产分支推送后检查 **绑定** 中的 `EMAIL`，用测试邮箱跑一次邮箱验证和密码重置。[接入发信域名](https://developers.cloudflare.com/email-service/get-started/send-emails/) · [邮件绑定](https://developers.cloudflare.com/email-service/configuration/send-bindings/)
- **仓库 Webhook**。在 Worker 的 **变量和密钥** 添加 Secret `WEBHOOK_ENCRYPTION_KEY`，值为 32 个随机字节的十六进制字符串，可用 `openssl rand -hex 32` 生成。部署后在 Shard 仓库「设置 → Webhook」录入接收端公网 HTTPS 地址和该订阅的签名密钥。备份时保留加密密钥。旧的实例级集成可继续用文本 `WEBHOOK_REPO`、`WEBHOOK_ENDPOINT` 和 Secret `WEBHOOK_SECRET`。[事件、签名与重试](../README.md#integrations)
- **仓库权限与 Git**。实例注册入口默认开放。创建业务仓库时选择可见性。在「账户设置 → 访问令牌」使用当前密码签发令牌。clone 可以使用具备仓库读取权限的令牌，push 需要写作用域和仓库写权限。Git 提示认证时用 Shard 用户名与令牌，令牌放在凭据管理器中。[Git 认证与作用域](../README.md#git-access)

## 升级、备份与上线验收

1. 从目标 Worker 的 **绑定**、**变量和密钥**、**域和路由** 记录实例用到的资源和密钥名称。暂停写入并等待相关后台作业结束，从同一时间点备份 D1、Git 和 LFS。把原库 UUID 填入所选 Wrangler 环境后，运行 `npx --no-install wrangler d1 export DB --remote --output shard.sql`，命名环境追加 `--env` 和原环境名。D1 的 **Time Travel** 可用于原库时间点恢复。离线备份应存放在私有位置。[D1 导出与恢复](https://developers.cloudflare.com/d1/best-practices/import-export-data/) · [D1 Time Travel](https://developers.cloudflare.com/d1/reference/time-travel/)
2. Free 实例保留同一 `GIT_REPOS` 命名空间。Artifacts 实例保留原命名空间与仓库 ID。原生 Git 同时备份持久盘。R2 Git 配对保存 D1、DO 的 refs 与 pack 清单和 `GIT_PACKS` 中的 pack 与索引。所有启用 LFS 的实例还需要保留 R2 中的 `lfs/` 键。Git mirror 可以保存导出 refs 可达的对象，历史 PR 快照可能需要原存储上的其他对象。[备份与恢复](../README.md#backup-and-restore) · [R2 Git 清单恢复](../README.md#r2-git-backup-and-restore)
3. 将新版本部署到原 Worker，保留所选环境的资源标识。先 dry-run，再用对应的命令发布或推送连接仓库的生产分支。更换存储后端时，把 Git refs 和 LFS 对象复制到目标后端的仓库，分别核对账号、权限、议题和 PR 等 D1 数据，再切换流量。修改 `GIT_BACKEND` 只影响新创建仓库的选型。
4. 部署后登录，创建一个测试私库，用写令牌 push，重新 clone 并运行 `git fsck --full`。检查原仓库、历史 PR 和已有 LFS 文件，验证无权限账号无法 clone 私库。启用了 LFS 的实例再测试上传与下载。自动更新场景可向部署仓库的生产分支推一笔预期变更，在 Cloudflare 确认运行的 Worker 版本并复查原有数据。

需回滚 Worker 时，打开 **部署 → 目标旧版本右侧的 ⋯ → 回滚（Rollback）**，并核对激活版本。Worker 版本回滚保留已应用的 D1 迁移，选择能读取现有 schema 的版本。备份恢复和独立 staging 验收见项目状态记录。[Dashboard 回滚](https://developers.cloudflare.com/workers/versions-and-deployments/rollbacks/#via-the-cloudflare-dashboard) · [项目验证记录](../STATUS.md#verification-log)

## 常见配置问题

| 现象 | 排查 |
| --- | --- |
| 推送后看不到构建 | 在 Worker「设置 → 构建」核对 Git 帐户、连接的仓库和生产分支，再看「部署」里的构建记录 |
| 私有仓库在选择器中缺失 | 检查 GitHub App 或 GitLab 的账号授权及仓库范围，再回到 Worker 的构建页重新选择 |
| 构建阶段找不到 `package.json` | 确认根目录是 `/`，且关联分支中包含 Shard 源码、锁文件和 `wrangler.jsonc` |
| D1 迁移失败 | 核对构建令牌的 D1 Edit、目标账号、`DB` 绑定和 UUID。查看日志里首个失败的迁移 |
| Worker 名匹配失败 | 让 Wrangler 顶层或所选环境的 `name` 与已连接的 Worker 一致，核对部署命令的 `--env` |
| 首次部署提示缺少 Secret | 在所选环境的 Worker 添加所需 Secret，或使用正确的 `--secrets-file` 首发 |
| Wrangler 提示 `native` 缺少 `MAX_PUSH_MB`、`SHA1DC` 或 `GIT_REPOS` | 这些键配置在 Free 的 SQLite Git 后端。`native` 使用 `NATIVE_GIT_ORIGIN` 和 `NATIVE_GIT_TOKEN`，在它自己的环境核对这两个值与 `DB` 绑定 |
| 构建成功但页面报错 | 对比激活版本与预期提交，打开 Worker 的 Observability 日志，用请求 ID 定位错误，检查 `DB` 与 Git 绑定 |
| LFS 上传出错 | 核对 `LFS_OBJECTS`、同桶 S3 endpoint、桶级凭据、模式、对象大小和客户端适配器 |
| 普通 Git push 过大 | 核对压缩后 pack、后端的单次 push 上限和域名入口限制。按可用的存储方案选择 LFS、R2 Git 分片或原生直连 |
