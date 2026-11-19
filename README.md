<h1><img src="assets/readme/shard.svg" width="100%" alt="Shard, a self-hosted Git forge on Cloudflare Workers"></h1>

Shard brings public and private repositories, issues, and pull request review
together for personal projects and small teams. Standard Git clients clone,
fetch, and push over HTTPS. The web interface supports English and Simplified Chinese.

[![Deploy to Cloudflare Free](https://deploy.workers.cloudflare.com/button)][cloudflare-deploy] · [Deploy Paid with R2 multipart uploads](#deploy-paid-with-r2-multipart-uploads)

[Run locally](#local-development) · [First push](#first-repository) · [Deploy guide](#deploy-to-cloudflare) · [简体中文](#简体中文)

<a href="assets/readme/pull-request.webp"><img src="assets/readme/pull-request.webp" width="100%" alt="Shard pull request with a versioned comparison, changed files, and a review comment linked to an added line"></a>

A line review in `demo/field-notes` on a local Shard instance.

## Local development

Install Git 2.32+ and mise, then run these commands from the checkout.

```sh
mise install
mise run setup
mise run dev
```

Open <http://localhost:8787>. Wrangler runs D1 and SQLite Durable Objects locally.
You can create and push a repository without a Cloudflare account.

### First repository

1. Create an account at `/user/sign_up`. Registration is open on the instance.
2. Choose **New repository** and set its visibility. Create a write token in
   **Account settings → Access tokens** and save it when Shard displays it.
3. Copy the repository's clone URL into `SHARD_CLONE_URL`. When Git prompts for
   the push, enter your Shard username and use the write token as the password.

```sh
git clone "$SHARD_CLONE_URL" my-repository
cd my-repository
printf '# My repository\n' > README.md
git add README.md
git commit -m "Initial commit"
git push -u origin HEAD:main
```

### Registration and administrators

Registration is open by default. Set the text variable
`SHARD_REGISTRATION=closed` to disable it; the signup page and guest
navigation then show a closed-registration message and `POST /user/sign_up`
returns 403. Existing accounts can still sign in.

Grant the first administrator from an existing account:

```sh
npm run admin -- list --env r2
npm run admin -- grant <username> --env r2
```

Use `--config PATH` for a non-default Wrangler configuration and `--local` for
the local D1 database. `grant --replace` demotes every other administrator
first. Administrators pass repository administration checks across the
instance, so grant only trusted accounts. Revoke with
`npm run admin -- revoke <username> --env r2`.

<details>
<summary>Build, tests, and editor setup</summary>

`mise run check` type-checks the Worker and `mise run lint` runs Clippy.
`mise run build` produces `build/index.js`. `mise run test` builds and tests
all three backends with isolated data. `npm test` uses an existing build.
Keep Git on `PATH`, including in CI.

`mise run setup` installs the pinned toolchain, editor tools and npm dependencies.
For rust-analyzer, set the Cargo target to `wasm32-unknown-unknown`.

Native merge regressions require Git 2.38 or later. When Git LFS is installed,
the LFS regressions also exercise uploads and clones with the real client.
</details>

## Features

- **Browse and search code** by Git ref. Find paths, text, and commits within a
  bounded scan. Results link to the searched commit. View a file, stream the
  full contents, or download a commit-pinned source archive.
- **Collaborate** with scoped Git tokens, repository collaborators, organization
  teams, forks, imports, and transfers. Existing URLs keep their permission
  checks after rename or transfer.
- **Review and merge** pull requests from a branch or another repository.
  Accepted pushes schedule a comparison refresh. Reviews and line comments
  link to their original snapshot. Protected branches can require approvals
  and CI results before the merge queue advances the target ref.
- **Organize work** with issue templates, reactions, label and assignee filters,
  milestones, repository watches and stars, and a permission-aware inbox.
  Repository writers can draft and publish releases pinned to Git tags.
- **Integrate** CI results and signed repository webhooks for issues, pull
  requests, branch updates, and tag updates.
- **Customize your profile** with an uploaded avatar. Commit lists, branch
  summaries, and commit details use it when the Git author email matches
  your account email. Other authors use their initials.
- **Verify GPG-signed commits** against the committer's registered public key.
  Commit details show the result and the key fingerprint.

## Architecture

The Rust Worker serves the web interface and checks repository access before
forwarding Git traffic. D1 keeps accounts, permissions, issues, and reviews.
The default deployment gives each repository a SQLite Durable Object for Git
objects and refs. Paid deployments can use Cloudflare Artifacts, and the
`native` environment uses an authenticated Git server with persistent disk.
It can expose a direct Git hostname for large transfers while the Worker
continues to authorize clone, fetch and push.
The `r2` environment runs native Git in Cloudflare Containers, stores immutable
Git packs and indexes in private R2, and publishes refs transactionally in
Durable Objects.
Any environment with a private `LFS_OBJECTS` R2 binding can store Git LFS bytes.

[Storage design](PLAN.md#optional-git-backends) · [Verified behavior and staging status](STATUS.md)

## Deploy to Cloudflare

Use the **Workers Free** deployment for a new forge. It provisions D1 and
SQLite Durable Objects. You can sign up, create public or private repositories,
push over HTTPS, open pull requests, and use issues and reviews immediately.

| Deployment | Git storage and push limit | Web merge | Git LFS | Services and account requirements |
| --- | --- | --- | --- | --- |
| Default (`shard-free`) | One SQLite Durable Object per repository, 32 MiB per push | Same-repository fast-forward | Optional R2 | Workers Free, D1, Static Assets, Rate Limiting, Cron |
| `npm run deploy:paid` (`managed` or `managed-staging`) | Artifacts, 64 MiB per push | Same-repository and cross-repository Artifacts merges through the Git container | R2 multipart | Workers Paid, Artifacts, Containers, Workflows, private R2, scoped S3 credentials |
| `npm run deploy:paid -- --env r2` (`shard-r2`) | R2 pack storage. Standard HTTPS within the domain limit and resumable multipart through `git-remote-shard` | Same-backend merges through the Git container | R2 multipart | Workers Paid, Containers, Durable Objects, Workflows, D1 and two private R2 buckets with scoped credentials |
| `--env artifacts` (`shard`) | Artifacts, 64 MiB per push | Same-repository fast-forward. Configure an external Git bridge for other Artifacts merges | R2 proxy by default | Workers Paid and R2. Preserves the original Artifacts instance's bindings |
| `--env native` (`shard-native`, hybrid) | Bare repositories on a Git server you operate. A direct Git hostname can route pack transfers to that server. | Full same-origin merges with Git 2.38+ | R2 proxy or multipart via `deploy:paid` | Workers Free or Paid, D1 and R2 on Cloudflare. Supply an HTTPS Git origin and persistent storage. |

Give each instance separate D1, Git storage, and any R2 resources.
See the [Chinese Cloudflare Dashboard manual](docs/configuration.zh-CN.md) for setup and upgrades, and the [deployment capacity guide](docs/deployment-capacity.md) for Git and LFS limits.
[Durable Object limits](https://developers.cloudflare.com/durable-objects/platform/limits/) · [Artifacts pricing](https://developers.cloudflare.com/artifacts/platform/pricing/) · [Enable R2](https://developers.cloudflare.com/r2/buckets/create-buckets/)

### Deploy from GitHub or GitLab

1. Click **Deploy to Cloudflare** and select your Cloudflare account.
2. Under **Set up your application**, select your GitHub or GitLab account
   and check **Create a private Git repository**. Confirm the Worker name
   and choose or create the D1 database for `DB`. Use a build token with
   **Workers Scripts Edit** and **D1 Edit** in the target account.
3. Keep the repository root as the build root, `npm run build` as the build
   command, and `npm run deploy` as the deploy command. Click **Deploy** and
   open the `workers.dev` URL when the build finishes.
4. Follow [First repository](#first-repository) to verify registration, repository
   creation and the first push.

Cloudflare records the Worker name and D1 ID in the new repository's
Wrangler configuration. Push changes to that repository's production branch
to update the Worker automatically. Each build runs the pending D1 migrations
against the selected database before publishing a Worker version tagged with
the Git commit SHA. See the build and deployed version under **Deployments**.
Keep these resource IDs when merging changes from a newer Shard release.

To connect a repository you already own, open the Worker's **Settings → Build**
and select the repository and its production branch. Use the same commands
and build token permissions. Add **Workers Routes Edit** when deploying routes
in a zone. Workers Builds uses `.node-version`, the pinned Rust toolchain and
`worker-build`.
[Deploy to Cloudflare buttons](https://developers.cloudflare.com/workers/platform/deploy-buttons/) · [Workers Builds settings](https://developers.cloudflare.com/workers/ci-cd/builds/configuration/)

### Deploy from a local checkout

Set up the toolchain from the local development section, then authenticate and
deploy from the checkout.

```sh
mise exec -- npx wrangler login
mise run deploy
```

The deploy script uses the selected environment's `DB` binding. With the
placeholder ID or no ID, it resolves D1 by `database_name` or creates it.
Automatic reuse accepts an empty database or a schema with Shard's initial
migration and core tables. To select an existing database explicitly, set its
UUID in `database_id`. The script applies pending migrations, then Wrangler
publishes the Worker with the selected bindings.
When a D1 jurisdiction is set, deployment checks it before reusing a database
by name and applies it when creating a database.

For another instance, choose a Worker and D1 name in `wrangler.jsonc`. Where
configured, choose unique R2 bucket and Artifacts namespace names. Pass
`--config PATH` to choose another config or `--env NAME` to select an
environment. Use the same Cloudflare account throughout. For CI or explicit
account selection, set `CLOUDFLARE_ACCOUNT_ID` and supply `CLOUDFLARE_API_TOKEN`
through the build environment. To check the bundle locally, run
`mise exec -- npm run deploy -- --dry-run`.
[Wrangler authentication](https://developers.cloudflare.com/workers/wrangler/commands/general/#login) · [D1 migrations](https://developers.cloudflare.com/d1/reference/migrations/) · [Wrangler resource provisioning](https://developers.cloudflare.com/workers/wrangler/configuration/#automatic-provisioning)

### Deploy Paid with R2 multipart uploads

Use the Paid preset from a source checkout or prebuilt release. Its default
target is `managed`. Use `--env managed-staging` for an independent staging
instance. Use Workers Paid with Artifacts and Containers access, an available
Docker engine, and a private R2 bucket. For a source checkout, install the
pinned toolchain as in [Local development](#local-development).

1. In the selected Wrangler environment, set the Worker name and the D1,
   Artifacts, Workflow and `LFS_OBJECTS` names for this instance. Create the
   private R2 bucket and a bucket-scoped **Object Read & Write** credential.
   Configure the `lfs-incoming/` rule in [Git LFS upload modes](#git-lfs-upload-modes).
2. Set `CLOUDFLARE_ACCOUNT_ID` in your shell and authenticate Wrangler. Save the
   credential in `.env.deploy.paid` with `SHARD_LFS_R2_ACCESS_KEY_ID` and
   `SHARD_LFS_R2_SECRET_ACCESS_KEY` as the two variable names.
3. Run this from a source checkout. In a prebuilt release, run the same command
   without `mise exec --`, using the included Node version.

   ```sh
   mise exec -- npm run deploy:paid -- --secrets-file .env.deploy.paid
   ```

The command derives the R2 S3 endpoint from the account and bucket jurisdiction,
configures the Artifacts origin, applies D1 migrations and deploys with
`SHARD_LIMIT_PROFILE=paid`. It defaults `SHARD_LFS_UPLOAD_MODE` to
`r2-multipart`; set that variable in the environment's `vars` (for example to
`r2-direct` for stock Git LFS clients) to keep the explicit mode. Run
`npm run deploy:paid -- --dry-run` to bundle locally without Cloudflare credentials.
The preset gives the Worker up to 300,000 ms of CPU and 20,000 subrequests
per invocation by default. Configure the [Git LFS transfer adapter](#git-lfs-upload-modes)
in each client checkout that uploads LFS files.

For repositories that need larger Git pushes, select the [Paid native setup](#large-repositories-on-workers-paid).

### Upgrade an existing Artifacts deployment

Use `npm run deploy -- --env artifacts` for the original Artifacts instance
or set that command in its existing Workers Builds configuration. Before
upgrading, carry the exact Worker name, D1 `database_id`, Artifacts namespace,
R2 bucket, routes and optional bindings from that instance into `env.artifacts`.
The migration keeps each repository's existing Git backend, storage ID, and
LFS keys. Use separate resources for a fresh Free deployment.

### Deploy with managed Cloudflare services

The `managed` and `managed-staging` environments add a Durable Object per target
branch, a repository copy Workflow, and a Containers image for Web merges that
need native Git. They use separate Workers, D1 databases, R2 buckets, Artifacts
namespaces, and Workflow names. Use Workers Paid with Artifacts and Containers
available in the target account.

The merge container uses `standard-1` by default (0.5 vCPU, 4 GiB RAM,
8 GB disk). Use `SHARD_GIT_MERGE_INSTANCE` to choose the runtime size.
Supported sizes include `lite`, `basic` and `standard-1` through `standard-4`.
[Instance sizes](https://developers.cloudflare.com/containers/platform/limits/#instance-types) · [Container pricing](https://developers.cloudflare.com/containers/platform/pricing/)

Before the first managed release, set `SHARD_GIT_ARTIFACTS_ORIGINS` in the
`vars` of both environments to the exact HTTPS Git origin for that account,
for example `https://<ACCOUNT_ID>.artifacts.cloudflare.net`. You can instead
set it as a text variable on each existing Worker. [Artifacts Git HTTPS](https://developers.cloudflare.com/artifacts/api/git-protocol/)

In the `shard-managed` Worker's **Settings → Build**, connect the repository
and keep the repository root as the build root. Configure the production branch
as `main` or set the build variable `SHARD_RELEASE_BRANCH` to your chosen
production branch. Use the following commands.

| Workers Builds setting | Command |
| --- | --- |
| Build command | `npm run ci -- --container` |
| Deploy command | `npm run cd` |

CI builds the Worker, runs the isolated regressions, and starts the Git bridge
image to check its health and authentication. CD deploys the checkout SHA to
staging, then waits for the release Workflow to verify the version tag, D1
schema, R2 access, CSS, and Git bridge readiness. Once staging passes, CD
deploys the same SHA to production and verifies it after activation. If the
publish or probe fails after this release activates its version, CD rolls
back to the previous single version when the deployment history still belongs
to this run. It checks the restored UUID and runs the previous version's probe
when that version has a SHA tag and the release Workflow binding.
Workers Builds reports the build and deploy result on the linked commit.
After deploying with the Paid preset, this staged release path retains its
dashboard-managed LFS settings through `keep_vars`. Use matching R2 settings
and secrets in both environments when staging multipart uploads.

Serialize writes to each production Worker during CD. Cloudflare's rollback
API has no conditional active-deployment check, so another publisher can race
the last check. A first release and a previous split deployment require
operator recovery. D1 migrations stay applied, so keep them compatible with
the previous Worker. A restarted Durable Object container uses the restored
Worker version's named image. Running containers keep their startup images.
[Worker rollback](https://developers.cloudflare.com/workers/versions-and-deployments/rollbacks/) · [Named container images](https://developers.cloudflare.com/containers/guides/image-management/#roll-out-a-named-image-update)

Workers Builds must be connected to the production Worker named in the
configuration. Staging uses its own Worker identity and bindings. Production
retains the identity check for the connected Worker.
[Migrate GitHub Actions jobs to Cloudflare](PLAN.md#migrate-github-actions-jobs-to-cloudflare)

If you enable builds for other branches, leave their preview command separate
from `npm run cd`. The release script checks the Workers Builds branch and
checkout SHA. Use a build token limited to the target account with Workers
Scripts Edit, D1 Edit and Workers R2 Storage Edit. Add Workers Routes Edit
when deploying custom routes. [Workers Builds settings](https://developers.cloudflare.com/workers/ci-cd/builds/configuration/) · [Container deployment](https://developers.cloudflare.com/containers/guides/deploy/) · [Workflow instance permissions](https://developers.cloudflare.com/api/resources/workflows/subresources/instances/methods/create/)

With Docker running, verify both bundles and their configured image from the
source checkout.

```sh
mise exec -- npm run cd -- --dry-run
```

For the first staged release, configure both environments and authenticate
Wrangler, then run the same commands Workers Builds uses. This creates the
Workers so you can connect `shard-managed` to the repository in the dashboard.

```sh
mise exec -- npm run ci -- --container
mise exec -- npm run cd
```

### Worker variables and secrets

In the Cloudflare dashboard, open **Workers & Pages → your Worker → Settings → Variables and Secrets**.
Choose **Add**, enter the name and value as **Text** or **Secret**, and deploy
the change. For a named Wrangler environment, open that environment's Worker,
such as `shard-native`. These are runtime settings. Workers Builds variables
configure the build process separately.

| Type | Settings |
| --- | --- |
| Text | `PUBLIC_ORIGIN`, `EMAIL_FROM`, `NATIVE_GIT_ORIGIN`, `NATIVE_GIT_DIRECT_ORIGIN`, `SHARD_REGISTRATION`, `SHARD_LIMIT_PROFILE`, `SHARD_LFS_UPLOAD_MODE`, `SHARD_LFS_MAX_BYTES`, `SHARD_LFS_R2_ENDPOINT`, `SHARD_LFS_R2_BUCKET`, `WEBHOOK_ENDPOINT` and `WEBHOOK_REPO` for the legacy integration |
| Secret | `NATIVE_GIT_TOKEN`, `SHARD_LFS_R2_ACCESS_KEY_ID`, `SHARD_LFS_R2_SECRET_ACCESS_KEY`, `WEBHOOK_ENCRYPTION_KEY`, `WEBHOOK_SECRET` for the legacy integration |

The shipped `keep_vars: true` preserves dashboard variables across Wrangler
deployments. For each dashboard-managed text variable, omit the corresponding
key from Wrangler `vars` in that environment. Values declared in `vars` are
uploaded on every deploy. Wrangler retains existing secrets, and you can
supply required secrets on a first deployment with `--secrets-file PATH`.
The file accepts JSON or dotenv. Files named `.env.deploy` and
`.env.deploy.*` are ignored by Git.
Keep resource bindings, routes and Cron Triggers in the Wrangler configuration.
[Dashboard environment variables](https://developers.cloudflare.com/workers/configuration/environment-variables/#add-environment-variables-via-the-dashboard) · [Workers secrets](https://developers.cloudflare.com/workers/configuration/secrets/#secrets-on-deployed-workers) · [Wrangler configuration precedence](https://developers.cloudflare.com/workers/wrangler/configuration/#source-of-truth)

### Deploy with a native Git origin

Choose the `native` Wrangler environment to use a Git server you operate. It
uses D1, R2, static assets and a rate limiter on Cloudflare, and stores bare Git
repositories on the origin's disk. Repository paths and Shard token scopes
follow the same conventions across backends. `GIT_BACKEND` selects storage
for new repositories and credential-free imports. A fork uses its source's backend.
Existing repositories retain their recorded backend.

1. Run `node scripts/git-origin.mjs` on a host with the pinned Node version and
   Git 2.32 or later. Set `SHARD_GIT_DIRECTORY` to a dedicated directory on a
   persistent volume and `SHARD_GIT_TOKEN` to a randomly generated shared
   secret. The service defaults to `127.0.0.1:8793`. Run one process per volume
   and put it behind an HTTPS reverse proxy that streams request bodies.
2. In `env.native.vars`, set `NATIVE_GIT_ORIGIN` to the proxy's HTTPS origin.
   To manage it through the dashboard on an existing Worker, remove the
   loopback entry from the file and set the dashboard variable on that Worker.
   Use a dedicated origin directory and token per instance.
3. For a new Worker, put `NATIVE_GIT_TOKEN` in `.env.deploy.native` with the
   same value as `SHARD_GIT_TOKEN`. Deploy the native environment with that file.

   ```sh
   mise exec -- npm run deploy -- --env native --secrets-file .env.deploy.native
   ```

   On subsequent deploys, omit `--secrets-file` to use the existing Worker secret.

For Windows or Linux local development, start the origin with a disposable
directory and token. Put `NATIVE_GIT_TOKEN` in `.dev.vars.native` and run:

```sh
mise exec -- npx wrangler d1 migrations apply DB --env native --local
mise exec -- npx wrangler dev --env native --local
```

Native imports accept final HTTPS Git URLs from `github.com`, `gitlab.com`,
and `codeberg.org` by default. Set `SHARD_GIT_IMPORT_HOSTS` on the origin to
replace the list with trusted hostnames. The import form accepts Basic or
Bearer credentials. Credentialed copies use the configured native origin even
when Artifacts is the default. They start as private repositories, and the
minute cron advances pending copies.

Back up the origin volume separately from D1. To keep serving legacy Artifacts
repositories, retain the Artifacts binding alongside the native origin until
those repositories are migrated. Native non-fast-forward Web merges require
Git 2.38 or later.
[Wrangler environments](https://developers.cloudflare.com/workers/wrangler/environments/) · [Git backend plan](PLAN.md#optional-git-backends)

### Large repositories on Workers Paid

Use the native Git origin when a push or repository exceeds Artifacts capacity.
The Paid preset deploys the Shard Worker and configures R2 multipart uploads.
Run the Git origin on a host you operate with a persistent volume and a
dedicated HTTPS hostname. For a hostname in Cloudflare DNS, use **DNS only**
to send Git packs directly to that host. A proxied hostname is subject to the
zone's request-body limit. The reverse proxy must stream uploads and downloads
and accept the intended pack size. With Nginx, use these settings inside the
TLS server for the Git hostname.

```nginx
location / {
    client_max_body_size 0;
    client_body_timeout 1h;
    proxy_http_version 1.1;
    proxy_request_buffering off;
    proxy_buffering off;
    proxy_read_timeout 1h;
    proxy_send_timeout 1h;
    proxy_set_header X-Real-IP $remote_addr;
    proxy_pass http://127.0.0.1:8793;
}
```

1. Configure the origin as in [Deploy with a native Git origin](#deploy-with-a-native-git-origin).
   Set `SHARD_GIT_FORGE_ORIGIN` on the Git host to the Shard Worker origin,
   such as `https://forge.example.com`. Set `SHARD_GIT_TRUST_PROXY=1` when
   the service listens on loopback behind a proxy that replaces `X-Real-IP`.
2. Set both `NATIVE_GIT_ORIGIN` and `NATIVE_GIT_DIRECT_ORIGIN` in
   `env.native.vars` to the Git origin, such as `https://git.example.com`.
   Set `CLOUDFLARE_ACCOUNT_ID` and use a private `LFS_OBJECTS` bucket with
   the R2 multipart [setup and lifecycle rule](#git-lfs-upload-modes).
3. Put `NATIVE_GIT_TOKEN` and the two bucket-scoped R2 credentials in
   `.env.deploy.paid`. Use the origin's `SHARD_GIT_TOKEN` as the value of
   `NATIVE_GIT_TOKEN`. Deploy the Paid native preset.

   ```sh
   mise exec -- npm run deploy:paid -- --env native --secrets-file .env.deploy.paid
   ```

Copy the clone URL from the repository page. For an existing checkout, update
the remote with `git remote set-url origin "$SHARD_CLONE_URL"`. Standard Git
clients send pack bytes to native Git. The Worker resolves the repository and
checks its current access rules. Before Git publishes refs, the Worker checks
write access and branch protection again. Accepted ref changes feed pull
request refresh and webhooks. Git LFS Batch stays authenticated through Shard,
and the multipart adapter uploads the object parts directly to R2.

The Git host's disk, resources, TLS proxy and Git limits determine achievable
pack size and repository capacity. Keep the Git volume backed up with D1 and
R2. This option serves repositories already recorded as native, including
new repositories in `env.native`. Existing Artifacts repositories retain
their recorded backend and Artifacts limits.
[Cloudflare upload limits](https://developers.cloudflare.com/workers/platform/limits/#request-and-response-limits) · [DNS proxy status](https://developers.cloudflare.com/dns/proxy-status/) · [Git receive configuration](https://git-scm.com/docs/git-config#Documentation/git-config.txt-receivemaxInputSize)

### Large repositories on Cloudflare R2

The `r2` environment keeps regular Git objects on Cloudflare. Containers run
native Git in disposable workspaces and read existing immutable packs from a
read-only R2 mount. A repository's Durable Object publishes the pack list and
refs in one transaction. Forks copy immutable packs to independent R2 keys.

When at least eight packs are present, background maintenance can combine up
to 16 small pack/index pairs within a 256 MiB input budget. Git keeps all their
objects, including commits outside current refs. The Durable Object switches
the manifest only after the new pair is stored and the repository revision
still matches. Replaced objects stay in R2 for 24 hours for older readers.
Large packs still count toward the workspace limit. [R2 Git backup](#r2-git-backup-and-restore)

1. Enable Workers Paid and Containers. In `env.r2`, set a separate Worker, D1
   database, and private R2 buckets `GIT_PACKS` and `LFS_OBJECTS`. Build the
   image using Docker.
2. Set `CLOUDFLARE_ACCOUNT_ID`. Put `SHARD_GIT_R2_ACCESS_KEY_ID` and
   `SHARD_GIT_R2_SECRET_ACCESS_KEY` in `.env.deploy.r2` with Object Read & Write
   scoped to `GIT_PACKS`. Add `SHARD_LFS_R2_ACCESS_KEY_ID` and
   `SHARD_LFS_R2_SECRET_ACCESS_KEY` scoped to `LFS_OBJECTS`.
3. Apply a seven-day expiration and incomplete multipart abort rule to
   `git-incoming/` in `GIT_PACKS` and the separate `lfs-incoming/` lifecycle
   rule to `LFS_OBJECTS`. Keep `git-packs/` available for repository reads.
4. Deploy the R2 Git environment.

   ```sh
   mise exec -- npm run deploy:paid -- --env r2 --secrets-file .env.deploy.r2
   ```

New repositories use R2 Git. Existing repository records keep their recorded
storage backend. To migrate a repository, mirror its Git refs to a new R2 Git
repository and transfer its Git LFS objects with the LFS client.

Standard Git clone, fetch and push use the HTTPS URL from the repository page.
For pushes exceeding the domain's request body limit, install the Shard Git
remote helper from the source checkout or prebuilt release package, then set a
separate push URL.

```sh
npm install -g .
git remote set-url --push origin 'shard::https://forge.example.com/owner/repo.git'
git push origin main
```

For a checkout that uses Git LFS, set its LFS push URL to the HTTPS endpoint
and configure the [LFS multipart adapter](#git-lfs-upload-modes).

```sh
git config remote.origin.lfspushurl 'https://forge.example.com/owner/repo.git/info/lfs'
```

The helper uses Git credentials for the HTTPS URL and sends pack parts to
signed R2 URLs. Shard lists the parts already in R2 and verifies the entire
assembled request before publishing. The server retains upload sessions and
the final Git report for six days from session creation. Resume from the
original checkout, where the helper saves the exact receive-pack request.

```sh
git remote-shard --list https://forge.example.com/owner/repo.git
git remote-shard --resume <spool-id> https://forge.example.com/owner/repo.git
```

Use `--abort` with the same arguments to cancel an unfinished server upload
and remove its local spool. Use `--discard` to remove only the local spool.
Keep enough space in the checkout's Git common directory for one push spool.
The server checks current write access, branch protection and the expected ref
state during publication. A stale ref result requires a fresh `git push`.

`SHARD_GIT_R2_MAX_PUSH_BYTES` defaults to 2 GiB for the receive-pack request,
with 32 MiB multipart parts that grow when needed to fit R2's part count.
Standard HTTPS also follows the Cloudflare zone's request body limit. The
`SHARD_GIT_R2_INSTANCE` variable selects the container type and defaults to
`standard-4`. Size it for Git's working files and incoming packs. Refs and
pack metadata have a per-workspace control budget of 4 MiB. Git pack bytes
stay in R2 across container restarts. Account for pack index and range-read
costs when sizing an instance.
[Workers limits](https://developers.cloudflare.com/workers/platform/limits/#request-and-response-limits) · [Container sizes](https://developers.cloudflare.com/containers/platform/limits/) · [R2 limits](https://developers.cloudflare.com/r2/platform/limits/)

Run the R2 Git integration test on Linux, WSL2 or macOS with a local
FUSE-enabled Docker engine. Build the Worker first.

```sh
mise run build
SHARD_TEST_R2_CONTAINER=1 mise exec -- node --test tests/r2-git.test.mjs
```

The test relay defaults to `host.docker.internal` for container requests.
Set `SHARD_TEST_R2_RELAY_HOST` to the Docker bridge gateway if the runner is
reachable through that address.
[Local Container development](https://developers.cloudflare.com/containers/guides/local-dev/#fuse-support)

### Web merges on Artifacts

Artifacts supports same-repository fast-forwards through Git HTTPS. Managed
environments send divergent and cross-repository Artifacts merges to the Git
merge container. Other Artifacts deployments can point `NATIVE_GIT_ORIGIN`
and `NATIVE_GIT_TOKEN` at a Git 2.38+ origin and set
`SHARD_GIT_ARTIFACTS_ORIGINS` on that host. The bridge stages the reviewed
objects and pushes one branch with a lease on the expected target tip.
Mixed-backend Web merges require manual integration and verification.

### Optional email setup

Set `PUBLIC_ORIGIN` to the HTTPS origin of your instance and `EMAIL_FROM` to an
address on your onboarded sending domain. Add `send_email` to the selected
Wrangler environment with `[{ "name": "EMAIL", "allowed_sender_addresses":
["shard@example.com"] }]`, using your own sender address. Verification links
expire after 24 hours. Password-reset links expire after 30 minutes. A
successful reset revokes every browser session and Git access token for the
account, including legacy tokens. Issue new tokens for Git clients and automation.
[Email Sending and domain onboarding](https://developers.cloudflare.com/email-service/get-started/send-emails/) · [`send_email` binding](https://developers.cloudflare.com/email-service/configuration/send-bindings/)

### Cloudflare services used by Shard

| Service | Role | Official documentation |
| --- | --- | --- |
| Workers and Static Assets | Serve the Rust/Wasm Worker and static files through `ASSETS`. | [Workers](https://developers.cloudflare.com/workers/) · [Static Assets binding](https://developers.cloudflare.com/workers/static-assets/binding/) · [Custom domains](https://developers.cloudflare.com/workers/configuration/routing/custom-domains/) |
| D1 | Store accounts, permissions, repository metadata and review history in `DB`. | [D1](https://developers.cloudflare.com/d1/) · [Migrations](https://developers.cloudflare.com/d1/reference/migrations/) |
| Artifacts | Store Git repositories in the Artifacts and managed environments through `ARTIFACTS`. | [Artifacts](https://developers.cloudflare.com/artifacts/) · [Namespaces](https://developers.cloudflare.com/artifacts/concepts/namespaces/) |
| R2 | Keep Git LFS objects private in `LFS_OBJECTS`. | [R2 Worker API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/) · [S3 credentials](https://developers.cloudflare.com/r2/api/tokens/) |
| Rate Limiting | Enforce per-source limits through `REQUEST_LIMITER`. | [Rate Limiting binding](https://developers.cloudflare.com/workers/runtime-apis/bindings/rate-limit/) |
| Cron Triggers | Process queued work every minute and expire records at 03:00 UTC. | [Cron Triggers](https://developers.cloudflare.com/workers/configuration/cron-triggers/) |
| Workers Logs | Record Worker errors and application logs. | [Workers Logs](https://developers.cloudflare.com/workers/observability/logs/workers-logs/) |
| Email Service | Send account verification and password reset links through optional `EMAIL`. | [Email Sending](https://developers.cloudflare.com/email-service/get-started/send-emails/) |
| Durable Objects | Store Free Git repositories in `GIT_REPOS` and coordinate protected merges in the managed environments. | [SQLite storage](https://developers.cloudflare.com/durable-objects/api/sqlite-storage-api/) · [Alarms](https://developers.cloudflare.com/durable-objects/api/alarms/) |
| Workflows | Resume repository copies and verify each managed release. | [Workflows](https://developers.cloudflare.com/workflows/) |
| Containers | Run Git for divergent and cross-repository Artifacts merges. | [Containers](https://developers.cloudflare.com/containers/) |
| Workers Builds | Build and test commits and run the staged release command. | [Workers Builds](https://developers.cloudflare.com/workers/ci-cd/builds/) |

### Deployment limits

The default and native Wrangler environments use `SHARD_LIMIT_PROFILE=free`.
Artifacts and managed environments use `paid`. On a Workers Paid account, a
native deployment can set `env.native.vars.SHARD_LIMIT_PROFILE=paid` to use
larger processing budgets. The deploy script defaults that environment to
300,000 ms of Worker CPU and honors an explicit `limits` override. The legacy
Artifacts environment configures 300,000 ms of CPU. Managed Workers also
configure 20,000 subrequests per invocation.

The SQLite backend streams pushes and copies to storage. `MAX_PUSH_MB` sets a
limit in MiB from 1 to 32, with 32 as the default. Push accounting includes the
receive commands and pack. Each fork or import must fit a single packed
snapshot, with at most 2,048 refs and 1 MiB of ref metadata. Import transfers
have a 60-second deadline. Expanded objects still consume isolate memory and
Durable Object storage.

Cloudflare's limits table lists 10 GB per SQLite Durable Object and 5 GB
total per Workers Free account. Its storage-full FAQ lists a Free-specific
limit of 1 GB per object.
[SQLite limits table](https://developers.cloudflare.com/durable-objects/platform/limits/#sqlite-backed-durable-objects-general-limits) · [Storage-full FAQ](https://developers.cloudflare.com/durable-objects/platform/limits/#what-happens-when-a-durable-object-exceeds-its-storage-limit)

The SQLite backend's Web UI supports same-repository fast-forwards. For a
divergent or cross-repository change targeting a protected branch, merge
locally on top of the current target tip. Push the result to a topic branch
in the target repository and open a fast-forward pull request.

On the SQLite Git backend, keep each ref name distinct from its ancestors.
For example, Git clients using file-based refs can fail to fetch when a
repository has both `refs/heads/topic` and `refs/heads/topic/notes`.
Shallow clones support numeric `--depth` on this backend.

Free imports accept credential-free HTTPS Git URLs on `github.com`,
`gitlab.com` and `codeberg.org`. Use the native origin for imports that
require credentials.

Artifacts enforces 64 MiB and 50,000 Git objects per push, with up to 32 MiB
per normal Git file. Repositories created on or after September 29, 2026 have
a 1 GB storage cap. Older repositories retain 10 GB. The same rules apply to
managed and legacy Artifacts. Split compatible pushes by commit or put large
files in LFS. Use the [direct native origin](#large-repositories-on-workers-paid)
when the repository or a Git transfer needs to exceed those ceilings.
[Artifacts limits](https://developers.cloudflare.com/artifacts/platform/limits/) · [Worker request-body limits](https://developers.cloudflare.com/workers/platform/limits/#request-and-response-limits)

The minute cron uses the following budgets. S3-backed LFS copies have a
smaller recovery page so multipart copy requests leave room for the other
jobs in the same invocation.

| Work | `free` | `paid` |
| --- | ---: | ---: |
| Repository copy checks per tick | 2 | 12, or 3 with an S3 endpoint |
| Repository cleanups per tick | 1 | 12 |
| R2 objects per Web or Workflow copy page | 2 | 8 |
| R2 objects per cron copy page | 2, or 1 with S3 | 16, or 1 with S3 |
| R2 keys per cleanup page | 256 | 1,000 |
| Unfinished push checks per tick | 1 | 24 |
| PR refreshes per tick | 1 | 32, or 28 with S3 |
| Merge queue entries per tick | 1 | 12 |
| Webhook deliveries per tick, per delivery mechanism | 1 | 32 |

Code search on `paid` scans up to 4,096 entries and 2 MiB of file content.
Commit search scans up to 64 first-parent commits. The `free` profile scans
up to 2,048 entries, 512 KiB of file content and 24 commits. Snapshot-based
PR comparisons on `paid` can read up to 160 trees and inspect 8,192 entries.
Native comparisons between repositories on the same origin use the origin's
Git command with a five-second comparison deadline and 256 changed files.

With `LFS_OBJECTS` bound, the default LFS mode (`proxy`) streams uploads through
the Worker. Its `SHARD_LFS_MAX_BYTES` default is `100000000`. The `free` profile
can lower it. The `paid` profile can raise it up to the lower of the domain's
request-body limit and R2's single-PUT limit of `5363466240` bytes. Cloudflare
Free and Pro zones accept 100 MB and Business zones accept 200 MB. The R2 S3
modes use the [limits below](#git-lfs-upload-modes).

Workers Free has 10 ms of CPU per HTTP or cron invocation and 50 external
subrequests per invocation. D1 Free allows 50 queries per Worker invocation
and 100,000 written rows per day across the account. Workers and R2 usage
follow their account and zone settings when you change the Shard profile.
[Workers limits](https://developers.cloudflare.com/workers/platform/limits/) · [D1 limits](https://developers.cloudflare.com/d1/platform/limits/) · [R2 limits](https://developers.cloudflare.com/r2/platform/limits/)

An HTTP Worker can stream while the client remains connected. Runtime updates
can close old connections at the end of a 20-minute drain window. `waitUntil()`
allows up to 30 seconds after the response or disconnect. An HTTPS origin
behind Cloudflare's proxy has a default 125-second read timeout and 30-second
write timeout. Each proxied LFS request has a 15-minute upload window. R2 S3
modes send upload bytes directly to R2. Shard records unfinished Git pushes
in D1 so Cron can reconcile them after a disconnect.
[Worker duration](https://developers.cloudflare.com/workers/platform/limits/#duration) · [Proxy timeouts](https://developers.cloudflare.com/fundamentals/reference/connection-limits/)

## 简体中文

Shard 是部署在 Cloudflare 上的自托管 Git 平台，适用于个人项目和小团队。D1 保存账户、权限、议题和审查记录。默认 Free 部署使用 SQLite Durable Objects，也可选择 Artifacts、Cloudflare 托管的 R2 Git 或自有原生 Git 源站。界面语言可在页脚或账号设置中切换。

[Cloudflare Dashboard 中文配置手册](docs/configuration.zh-CN.md) 按面板逐项说明部署参数、私有仓库自动更新、各环境的资源绑定、变量与密钥、域名和升级验证。[资源限制与扩容空间](docs/deployment-capacity.md) 详列 Git 与 LFS 的容量、分片和并发边界。

点击 **Deploy to Cloudflare**，在「设置您的应用程序」中选择 Git 帐户，勾选「创建专用 Git 存储库」，确认 Worker 和 D1 数据库后部署。Cloudflare 会绑定新仓库，之后推送到该仓库的生产分支就会自动更新同一个 Worker，并在原 D1 数据库上应用迁移。[Free 一键部署字段与步骤](docs/configuration.zh-CN.md#free-一键部署与自动更新)

在本地试用无需 Cloudflare 账号。安装 Git 2.32 或更高版本和 mise 后，运行 `mise install`、`mise run setup`、`mise run dev`，打开 `http://localhost:8787`。

线上变量和密钥可在 Cloudflare 的 **Workers & Pages → 对应 Worker → Settings → Variables and Secrets** 中设置。控制台维护的文本变量应从该环境的 Wrangler `vars` 中移除，默认部署会保留这些变量。[Dashboard 变量与密钥配置](docs/configuration.zh-CN.md#在-dashboard-添加运行时变量与密钥)

Paid 环境可运行 `npm run deploy:paid -- --secrets-file .env.deploy.paid`，默认启用 R2 分片上传。已有 Artifacts 实例通过 `--env artifacts` 升级并保留原来的资源绑定。自管 Git 服务器可使用 `--env native`。[Paid 部署说明](#deploy-paid-with-r2-multipart-uploads) · [原生 Git 部署说明](#deploy-with-a-native-git-origin)

大仓可运行 `npm run deploy:paid -- --env native --secrets-file .env.deploy.paid`，为原生 Git 源站配置 DNS only 的 HTTPS 域名和持久盘。标准 Git 直接向源站传输 pack，Worker 仍检查私库访问、写入令牌和分支保护。LFS 分片直接上传到 R2。[Paid 大仓部署](#large-repositories-on-workers-paid)

无需 VPS 的大仓可运行 `npm run deploy:paid -- --env r2 --secrets-file .env.deploy.r2`。R2 保存普通 Git 的 pack 和索引，容器按需运行 Git。标准 HTTPS 支持 clone、fetch 和域名限额内的 push，安装 `git-remote-shard` 可将大推送分片直传 R2，并从 R2 已接收的分片恢复。[R2 Git 部署与续传](#large-repositories-on-cloudflare-r2)

选择 `managed-staging` 和 `managed` 可用 Durable Objects 协调受保护分支的合并、用 Workflows 续跑仓库复制，并通过 Containers 执行需要原生 Git 的 Artifacts 合并。Workers Builds 使用 `npm run ci -- --container` 验证代码与镜像，再通过 `npm run cd` 部署 staging。发布 Workflow 验证通过后，同一提交才会部署到生产并再次验证。[托管 CI/CD 配置](#deploy-with-managed-cloudflare-services)

## Project releases

Add an exact `## <version>` section to [CHANGELOG.md](CHANGELOG.md) in the
tagged commit. Pushing `v0.2.0` publishes that version's notes. A prerelease
such as `v0.3.1-rc.1` requires its own `## 0.3.1-rc.1` section and is marked
as a prerelease on GitHub. Check the selected notes locally with
`mise exec -- node scripts/release-notes.mjs v0.3.1`.

The workflow checks the tag against the npm, Cargo, and built Worker versions.
It builds and tests both Git container images, dry-runs the extracted archive
with Free, Artifacts, native, managed, staging, and Paid presets including R2,
and invokes the installed Git helper. It verifies the archive's SHA-256 before
publishing the GitHub Release.

To deploy the prebuilt archive, verify its checksum, extract it, and use the
Node version in the included `.node-version`. Set the resource names in the
extracted `wrangler.json` and authenticate to the target Cloudflare account.
Run these commands from the extracted directory.

```sh
npm ci
npm run deploy
```

For native mode, use the packaged `scripts/git-origin.mjs` and the same
`--env native` and `--secrets-file` options as a source deployment.
For R2 Git, use `npm run deploy:paid -- --env r2 --secrets-file .env.deploy.r2`
with the [R2 Git bindings and credentials](#large-repositories-on-cloudflare-r2).
The archive includes the Git container build inputs and `git-remote-shard`.

[Release workflow](.github/workflows/release.yml) · [GitHub Releases](https://github.com/aliveranme/shard/releases)

## URL shape

Branch names contain slashes, so branches are a query parameter:

```
/{owner}/{repo}?branch=main
/{owner}/{repo}/src/{file-path}?branch=feature%2Freadme
/{owner}/{repo}/blob/{file-path}?branch=main
/{owner}/{repo}/raw/{file-path}?branch=main
/{owner}/{repo}/commits?branch=feature%2Freadme&start={commit-hash}
/{owner}/{repo}/refs
/{owner}/{repo}/commit/{commit-hash}
/{owner}/{repo}/archive/{commit-hash}
```

Repository and issue lists accept `page` (20 items per page); search and state
filters survive navigation. Discussion pages use `comments_page` and include
both comments and close/reopen events. Commit history follows first parents:
the `start` cursor identifies the first commit on the next page, so each request
reads at most 21 commits. “Latest commits” returns to the selected branch tip.

## Git access

Shard checks the Git client's Shard token and repository access, then streams
the Smart HTTP request and response through its recorded Git backend.
Artifacts requests use a short-lived repository token. Native origin requests
use the instance's shared secret. The Free Git backend is reached through a
private Durable Object binding. Native repositories can advertise a
[direct Git hostname](#large-repositories-on-workers-paid) for pack transfers.

Create tokens under `/user/settings/tokens` with your current password. Choose
read/write scope and a 7/30/90/365-day lifetime (30 days by default). In a
private repo, owners and collaborators with either role can clone using either
token scope. Pushing requires both a write-scoped token and repository write
permission. Expired or revoked tokens are rejected. Legacy tokens without an
expiry remain valid until revoked. The secret is shown only at creation. Use
it as the password when Git prompts. Anonymous clone works for public repositories.

The default branch must exist in Git. Changing it also changes the branch
new clones select through Shard. Use **Save and sync Git HEAD** to synchronize
an existing default. Durable Object and native storage write the symbolic ref
directly. Artifacts advertises the configured branch through Shard's upload-pack
proxy using the classic Git advertisement. Account credential changes require
the current password.
Before deleting an account, transfer or remove its repositories. Deletion
revokes the account's sessions and tokens.

Passwords use PBKDF2-SHA256. Browser sessions use HTTP-only cookies. Session and
token secrets are stored as hashes. Browser forms require session-bound CSRF
tokens and same-origin requests. Failed logins are limited to eight per
source/account pair within 15 minutes. Recovery pages send only the site origin
as referrer. Git smart HTTP returns an empty HTTP 404 for a missing `.git` repository.

### GPG commit signatures

In **Settings → GPG keys**, add the public certificate for your signing key
and confirm with your current password. Use your Shard account email as your
Git committer email and include it in the key's user IDs.

```sh
gpg --armor --export YOUR_FINGERPRINT
git config user.email YOUR_SHARD_EMAIL
git config user.signingkey YOUR_FINGERPRINT
git commit -S -m "Describe the change"
```

Shard verifies OpenPGP signatures over the original Git commit bytes. A valid
signature displays the primary key fingerprint. The page separately reports
whether the committer's email has been verified on Shard. Update an existing
key by uploading the refreshed certificate with the same fingerprint. Removing
it stops verification with that key. Each account can store up to eight
public keys, each up to 32 KiB.

The SQLite and native backends read the commit directly. Artifacts reads it
through a bounded Git fetch. Commit details show verification as unavailable
when the raw object cannot be read within the request budget, including
commits above 256 KiB. The verifier uses OpenPGP.js 6.3.2 under LGPL-3.0-or-later.
[OpenPGP.js source](https://github.com/openpgpjs/openpgpjs/tree/v6.3.2)

### Git LFS

Bind a private R2 bucket as `LFS_OBJECTS` to enable LFS in a Free installation.
Keep the binding and bucket contents when upgrading or restoring an LFS instance.

Install Git LFS and use the regular Shard HTTPS remote. `git lfs track` stores
pointers in Git and the Batch endpoint negotiates object transfers. Uploads
require a write-scoped token and repository write access. Downloads use the
Basic adapter in every mode. Public objects can be downloaded anonymously
and private downloads require a repository grant.

Each object is scoped to its repository and verified against its declared
SHA-256 and size. A batch can contain up to 100 objects. A fork publishes
after copying its LFS objects into an independent R2 namespace. Repository
deletion removes access immediately and schedules Git and R2 cleanup. Copy
LFS objects separately for external Git imports and mirrors.
[Git LFS Batch API](https://github.com/git-lfs/git-lfs/blob/main/docs/api/batch.md)

#### Git LFS upload modes

Set `SHARD_LFS_UPLOAD_MODE` through the [Worker's Variables and Secrets](#worker-variables-and-secrets)
or the selected environment's Wrangler `vars`. The deploy command applies D1
migration `0026_lfs_multipart.sql` when upgrading.

| Mode | Client | New object limit |
| --- | --- | --- |
| `proxy` (default) | Standard Git LFS | 100,000,000 bytes by default. The [deployment limit](#deployment-limits) also applies. |
| `r2-direct` | Standard Git LFS | 5,363,466,240 bytes per single PUT. A failed PUT restarts the object. |
| `r2-multipart` (Paid preset) | Git LFS with the Shard upload adapter | 5,363,466,240 bytes by default on both profiles. With `paid`, an explicit limit can reach the 5,492,189,429,760-byte R2 ceiling. |

Set `SHARD_LFS_MAX_BYTES` in bytes to tune the limit for new uploads. To resume
an unfinished upload above the new default after upgrading, set an explicit
cap that covers its size. Stored objects remain readable at a lower cap. Uploads
in both R2 modes go from the client to the R2 S3 endpoint and bypass the
Shard domain's request-body size limit. Multipart uploads can resume recorded
parts for up to six days. The complete operation verifies the file before
it becomes visible in the repository. With large objects above the single
PUT ceiling, the Worker streams the staged object through SHA-256 and uses
R2 multipart copy to publish it. Verification and publication each have a
15-minute wall-time window and share the invocation's CPU budget.
Set a lower `SHARD_LFS_MAX_BYTES` for the throughput verified on your instance.
Hosted verification and fork copying near the protocol ceiling remain unmeasured.

Both R2 modes use the following settings in the same Worker environment.
Set the endpoint and bucket as text variables in the dashboard or Wrangler.
Set the credentials as Worker secrets in the dashboard or a deploy secrets file.

| Setting | Value |
| --- | --- |
| `SHARD_LFS_R2_ENDPOINT` | The bucket's HTTPS S3 endpoint, such as `https://<account-id>.r2.cloudflarestorage.com` |
| `SHARD_LFS_R2_BUCKET` | The same private bucket as `LFS_OBJECTS` |
| `SHARD_LFS_R2_ACCESS_KEY_ID` | Worker secret for a bucket-scoped R2 Object Read & Write credential |
| `SHARD_LFS_R2_SECRET_ACCESS_KEY` | Worker secret for that credential |

Add a bucket lifecycle rule for `lfs-incoming/` that expires staged objects
and aborts incomplete multipart uploads after seven days. Set
`SHARD_LFS_R2_BUCKET` in your shell to the name configured for `LFS_OBJECTS`.
Check the rule in the R2 dashboard or list the policy before adding it. For
a jurisdictional bucket, pass its `--jurisdiction` value to both commands.
The example selects the Paid `managed` environment.

```sh
npx --no-install wrangler r2 bucket lifecycle list "$SHARD_LFS_R2_BUCKET" --env managed
# Run only if the equivalent rule is absent.
npx --no-install wrangler r2 bucket lifecycle add "$SHARD_LFS_R2_BUCKET" shard-lfs-staging lfs-incoming/ --expire-days 7 --abort-multipart-days 7 --env managed
```

Published objects live under `lfs/`. Keep the S3 settings available
after changing modes if repositories contain objects above the single-PUT
limit, since large fork copies still use the S3 API.
[R2 API credentials](https://developers.cloudflare.com/r2/api/tokens/) · [R2 lifecycle rules](https://developers.cloudflare.com/r2/buckets/object-lifecycles/)

For `r2-multipart`, install the pinned Node version and configure the
packaged `scripts/git-lfs-r2-multipart.mjs` in each Git checkout.

```sh
git config --local lfs.customtransfer.shard-r2-multipart.path node
git config --local lfs.customtransfer.shard-r2-multipart.args '"/absolute/path/to/scripts/git-lfs-r2-multipart.mjs"'
git config --local lfs.customtransfer.shard-r2-multipart.direction upload
```

Use the same `git push` and `git lfs push` commands after setup. The adapter
uses Shard only for control requests. It streams missing parts to R2 and
reports success after Shard verifies and publishes the assembled object.
[Git LFS custom transfers](https://github.com/git-lfs/git-lfs/blob/main/docs/custom-transfers.md)

## Images and issue references

An avatar can be a PNG, JPEG or WebP image up to 512 KiB and 1024 × 1024 px.
Shard saves a metadata-free PNG thumbnail up to 128 × 128 px in D1.

README files, Markdown file previews and discussions render Markdown images and
links. For example:

```markdown
![Field map](images/site-map.png)
[Survey notes](docs/survey.md#measurements)
![Public status](https://assets.example.org/status.png)

<img src="images/site-map.png" alt="Field map" width="320">

See #12, GH-12, and mira/field-journal#34.
```

Repository-relative URLs in a file use the displayed commit and resolve from that
file's directory. In a discussion they use the repository root on its default
branch. A line review uses the saved commit and directory for the reviewed side.
Repository media uses the same access checks as source files. The browser loads
external HTTPS images without a referrer. HTML image tags can use `src`, `alt`,
`title`, `width`, and `height`. Shard escapes other HTML tags and discards other
attributes from rendered images.

Issue references open the issue or pull request with that number in Shard.
Paste an HTTPS URL to link directly to a GitHub issue or pull request.

## Repository workflows

**Branches and tags** lets repository owners create a branch or lightweight tag
from a commit ID, or remove a ref after confirming its name. **Settings → Branch
protection** adds rules for exact branch names. Protected refs accept changes
through the PR merge queue. Required check results belong to the reviewed head
commit and the target repository. Once a comparison changes, review its new
version before queuing again.
Changing a branch rule during an admitted write or a running merge returns a
conflict. Retry after the write completes. If a branch write loses its Git
response, its admission expires after one hour. External Git backends can
finish an ambiguous write outside that window.

In the managed environments, an alarm wakes the branch queue on enqueue or
CI updates. A Workflow advances each repository copy through Git and LFS
publication. The minute cron recovers work when a wake or start is missed.

**Settings → Organizations** lets a repository owner offer the repository to an
organization. The organization owner accepts it and grants teams read or write
access. Repository ownership and its URL stay with the account that offered it.

Accepting a repository transfer disables repository-managed Webhooks, clears
their signing keys, and cancels outstanding deliveries. The recipient can see
delivery history and configure new subscriptions. A delivery already handed
to the network can still arrive. The instance-configured integration continues
to follow the repository. An active Git HEAD update blocks acceptance for
up to five minutes. If storage returned an ambiguous result, use **Save and
sync Git HEAD** to reconcile it after the update has finished.
Historical URLs remain usable while the former owner's account exists.

**Settings → Import metadata** accepts a version 1 JSON bundle for issues,
pull requests, and releases. The form includes the schema and size limits.
The destination Git store must contain the referenced commits and release tags.
Each bundle imports in one D1 transaction.

## Interface language

Open **Language** in the footer or **Account settings → Language**. Choose
English, 简体中文 (Simplified Chinese), or **Use browser language**.
Signed-in choices are stored in D1 and apply across sessions. If an account has
no saved preference, Shard uses the browser's language cookie, then its
`Accept-Language` priorities. Unsupported languages fall back to English.
The **Use browser language** setting explicitly selects the browser header.

The Worker translates Web UI labels, validation messages, errors, accessibility
labels, relative dates and counts. Repository contents, issues and commit
messages appear in the language their authors used.

To add a locale, register its tag and display label in `src/i18n/mod.rs`, add
its text to each catalog in `src/i18n/`, and add its formatting and negotiation
rules. Extend the D1 locale constraint with a forward migration. Keep
interpolated messages together so translators can change word order. Templates
escape the output, including interpolated values.

## Integrations

To record a check, POST JSON to `/{owner}/{repo}/commit/{hash}/checks` with
`Authorization: Bearer <token>` and a write-scoped token belonging to a repository
writer. Send `{ "context": "ci/unit", "state": "success", "summary": "42 tests passed" }`.
The accepted states are `pending`, `success`, `failure` and `error`. Each context
has one current result per commit. Results appear on the commit detail and the
corresponding pull request. A successful write returns `204`.
For a cross-repository pull request, a target writer with source access can
report a check for the saved head SHA at the target repository's endpoint.

To manage signed repository webhooks, configure `WEBHOOK_ENCRYPTION_KEY` as a
Worker secret containing 32 random bytes in hex, then open **Settings → Webhooks**.
Add an HTTPS URL on a public hostname and a signing secret. Each repository can
save up to 10 subscriptions. The minute cron retries a delivery up to six
attempts. Events are retained for 30 days and settings shows five recent
deliveries per subscription.

Verify `X-Shard-Signature` as `sha256=` followed by the lowercase hex HMAC-SHA256
of the exact request body. `X-Shard-Event-Id` identifies the event and
`X-Shard-Delivery-Id` identifies the subscription delivery across retries.
Git events include `ref`, `before`, and `after`. A missing tip is JSON `null`.
For a push through Shard, D1 records the proposed ref transitions before
forwarding the pack. A complete Git receive report commits the ref event,
Webhook outbox, and PR refresh together. If that report is lost, the minute
cron checks the live refs for up to one hour. A recovered observation has
an unknown actor. Intermediate updates superseded before observation can
be missed.

Existing instance-level `WEBHOOK_REPO`, `WEBHOOK_ENDPOINT`, and `WEBHOOK_SECRET`
configuration continues to deliver issue and review events.

## Backup and restore

Pause writes and scheduled cleanup while taking a paired D1 and Git backup.
Include the R2 `lfs/` prefix if LFS is enabled. Keep the backups private. Record
each repository's `id`, `git_backend`, stable storage ID (`artifacts_repo`),
and `lfs_copy_id` from D1.

For an in-place hosted D1 recovery, record a Time Travel bookmark while writes
are paused. Bookmarks cover seven days on Workers Free and 30 days on Workers
Paid. Keep a SQL export for recovery into a new database.

For a loopback rehearsal, set `SHARD_RESTORE_D1` to an empty persistence path
and `SHARD_BACKUP` to an absolute backup directory. Local D1 export reads
`.wrangler/state` next to the Wrangler configuration. If the source used
`--persist-to`, copy its `v3/d1` directory into `.wrangler/state/v3/` next to
a temporary copy of the configuration and pass that copy with `--config`.
For the native environment, add `--env native` to both D1 commands.

Set `SHARD_REPO_URL` to the clone URL for one repository with refs and
`SHARD_REPO_BACKUP` to its own absolute backup directory. Create both backup
directories before running these commands.

```sh
mise exec -- node node_modules/wrangler/bin/wrangler.js d1 export DB --local --output "$SHARD_BACKUP/shard.sql"
```

Copy `shard.sql` to `shard-restore.sql`. With Wrangler 4.164.0, move the
`CREATE VIEW git_ref_observations` statement before `CREATE TRIGGER git_ref_observe`
in the import copy. The local exporter writes triggers before views.

```sh
mise exec -- node node_modules/wrangler/bin/wrangler.js d1 execute DB --local --persist-to "$SHARD_RESTORE_D1" --file "$SHARD_BACKUP/shard-restore.sql"
git clone --mirror "$SHARD_REPO_URL" "$SHARD_REPO_BACKUP/source.git"
git -C "$SHARD_REPO_BACKUP/source.git" bundle create "$SHARD_REPO_BACKUP/source.bundle" --all
git clone --mirror "$SHARD_REPO_BACKUP/source.bundle" "$SHARD_REPO_BACKUP/restored.git"
git -C "$SHARD_REPO_BACKUP/restored.git" fsck --full
```

Compare D1 row counts and check that `git for-each-ref` returns the same names and
object IDs in both mirrors. For an empty repository, retain the mirror and its
D1 record. Restore storage to the expected IDs before serving the restored
database. For native storage, preserve the bare-repository volume, including
objects from deleted branches that archived pull request comparisons still use.
Free repositories require paired recovery of D1 and the same `GIT_REPOS`
namespace. A mirror preserves objects reachable from exported refs. Saved PR
comparisons can require objects outside those refs, so retain and verify the
original storage for those snapshots. Cloudflare provides per-object
point-in-time recovery for SQLite Durable Objects.
[Durable Object recovery](https://developers.cloudflare.com/durable-objects/api/sqlite-storage-api/#point-in-time-recovery-pitr)

For R2, configure private S3 remotes in rclone. Set `SHARD_R2_SOURCE` and
`SHARD_R2_RESTORE` to values of the form `remote:bucket`, pointing to the
source and an empty restore bucket. Use the same backup directory as the
D1 export and preserve the complete keys under `lfs/`, including
`lfs/<repo-id>/<oid>` and `lfs/fork-<copy-id>/<oid>`.

```sh
rclone copy "$SHARD_R2_SOURCE/lfs" "$SHARD_BACKUP/r2/lfs" --immutable
rclone check "$SHARD_R2_SOURCE/lfs" "$SHARD_BACKUP/r2/lfs" --download
rclone copy "$SHARD_BACKUP/r2/lfs" "$SHARD_R2_RESTORE/lfs" --immutable
rclone check "$SHARD_BACKUP/r2/lfs" "$SHARD_R2_RESTORE/lfs" --download
```

Bind the restored bucket with its matching D1 and Git backup. Clone a
repository and a fork with LFS and verify that saved PR revisions also open
before resuming traffic. [R2 with rclone](https://developers.cloudflare.com/r2/examples/rclone/)

[D1 export and import](https://developers.cloudflare.com/d1/best-practices/import-export-data/) · [D1 Time Travel](https://developers.cloudflare.com/d1/reference/time-travel/)

### R2 Git backup and restore

Back up D1, each R2 Git repository's manifest, and both R2 buckets from the
same quiet period. Pause ingress writes and drain scheduled cleanup and copy
jobs. Use a new private backup directory and save the D1 SQL export with
`wrangler d1 export DB --remote --env r2 --output "$SHARD_BACKUP/shard.sql"`.
Use the `lfs/` backup commands above.

Run the [recovery Worker](worker/git-r2-recovery.mjs) in a loopback Wrangler
remote development session. It binds to the deployed Shard Worker in the
same account. Keep the helper configuration and `.dev.vars` in a private
directory. Copy `worker/git-r2-recovery.mjs` there and use the following
configuration. Replace the database ID, database name and Worker script name
with your source instance's values.

```jsonc
{
  "name": "shard-r2-recovery",
  "main": "git-r2-recovery.mjs",
  "compatibility_date": "2026-11-17",
  "compatibility_flags": ["nodejs_compat"],
  "workers_dev": false,
  "preview_urls": false,
  "d1_databases": [{ "binding": "DB", "database_name": "shard-r2", "database_id": "YOUR_D1_UUID" }],
  "durable_objects": {
    "bindings": [{ "name": "GIT_R2_REPOS", "class_name": "R2GitRepository", "script_name": "shard-r2" }]
  }
}
```

Set `SHARD_RECOVERY_DIR` to that directory. Generate a fresh 64-character
hex token in its `.dev.vars` as `SHARD_R2_RECOVERY_TOKEN`. Start Wrangler
from the checkout or prebuilt package with its installed dependencies.

```sh
umask 077
printf 'SHARD_R2_RECOVERY_TOKEN=%s\n' "$(openssl rand -hex 32)" > "$SHARD_RECOVERY_DIR/.dev.vars"
npx --no-install wrangler dev --remote --config "$SHARD_RECOVERY_DIR/wrangler.jsonc" --ip 127.0.0.1 --port 8794
```

From another Bash shell with `jq`, set `SHARD_BACKUP` to the private backup
directory and `SHARD_GIT_SOURCE` to the Git pack bucket's `rclone` remote.
Export all pages of repository IDs and save their exact `.pack` and `.idx`
keys. Background compaction can retire old keys after 24 hours. Check that
every referenced key was copied before accepting the backup.

```sh
umask 077
shard_recovery_origin=http://127.0.0.1:8794
shard_recovery_token=$(cut -d= -f2 < "$SHARD_RECOVERY_DIR/.dev.vars")
shard_recovery() {
  printf 'Authorization: Bearer %s\n' "$shard_recovery_token" |
    curl --fail-with-body --silent --show-error --header @- "$@"
}
mkdir -p "$SHARD_BACKUP/manifests" "$SHARD_BACKUP/r2-git" || exit 1
: > "$SHARD_BACKUP/git-keys.txt"
after=
while :; do
  page=$(shard_recovery "$shard_recovery_origin/repositories?after=$after") || exit 1
  for id in $(jq -r '.repositories[]' <<< "$page"); do
    manifest="$SHARD_BACKUP/manifests/$id.json"
    shard_recovery "$shard_recovery_origin/repositories/$id" > "$manifest" || exit 1
    jq -r '.packs[] | "\(.).pack", "\(.).idx"' "$manifest" >> "$SHARD_BACKUP/git-keys.txt" || exit 1
  done
  after=$(jq -r '.next // empty' <<< "$page") || exit 1
  [ -n "$after" ] || break
done
rclone copy "$SHARD_GIT_SOURCE" "$SHARD_BACKUP/r2-git" --files-from "$SHARD_BACKUP/git-keys.txt" --immutable || exit 1
while IFS= read -r key; do
  [ -s "$SHARD_BACKUP/r2-git/$key" ] || { printf 'Missing Git backup object %s\n' "$key" >&2; exit 1; }
done < "$SHARD_BACKUP/git-keys.txt"
rclone check "$SHARD_GIT_SOURCE" "$SHARD_BACKUP/r2-git" --files-from "$SHARD_BACKUP/git-keys.txt" --one-way --download
```

For recovery, import the SQL dump into an empty D1 database and restore the
LFS objects to their original keys. Set up a recovery instance of the same
Shard release with its own Worker name, empty Durable Object namespace and
private buckets. Set its `DB` UUID explicitly and fill in the [R2 Git variables and secrets](docs/deployment-capacity.md#cloudflare-dashboard-从-git-部署).
Import with `wrangler d1 execute DB --remote --env r2 --file "$SHARD_BACKUP/shard.sql"`
against the destination configuration. Then publish the same Worker release
using `wrangler deploy --env r2` with the restored database and bucket bindings.
Verify the restore before opening traffic. Restart the helper with the
destination D1 and Worker names before importing any manifests.

Set `SHARD_GIT_RESTORE` to the destination pack bucket's `rclone` remote.
Copy and check the pack objects, then import each manifest. A first import
returns HTTP 201. Repeating the same manifest returns 204. A mismatched
existing repository returns 409 and a missing pair returns 422.

```sh
rclone copy "$SHARD_BACKUP/r2-git" "$SHARD_GIT_RESTORE" --immutable || exit 1
rclone check "$SHARD_BACKUP/r2-git" "$SHARD_GIT_RESTORE" --one-way --download || exit 1
for manifest in "$SHARD_BACKUP"/manifests/shard-*.json; do
  [ -f "$manifest" ] || continue
  id=$(basename "$manifest" .json)
  shard_recovery -X PUT --data-binary "@$manifest" "$shard_recovery_origin/repositories/$id" || exit 1
done
```

Before opening traffic, compare advertised refs and HEAD, clone the private
repository and a fork, and run `git fsck --full`. Check a saved review for a
branch that was deleted and download an LFS object from both repositories.
Stop the Wrangler helper when the rehearsal is complete. The backup represents
published repository data at its recovery point. Restart unfinished Git or
LFS uploads against the restored instance.
[Wrangler remote development](https://developers.cloudflare.com/workers/local-development/bindings-per-env/#remote-development) · [Durable Object environment bindings](https://developers.cloudflare.com/durable-objects/reference/environments/)

Worker responses include `X-Request-ID` for log correlation. Daily maintenance
at 03:00 UTC removes expired sessions, tokens, login throttle windows, and email links.

## Limits

- Pull request comparisons report missing Git objects and traversal limits.
  Shard requires a unique best common ancestor and reports unrelated branches
  or multiple best common ancestors explicitly. Text diffs cover UTF-8 files
  up to 96 KiB and 512 lines per side. Same-origin native repositories have
  one authenticated origin request per merge-base, file-tree comparison, or
  ancestry-verification step, with a five-second Git operation budget and
  up to 256 changed files. Durable Object, Artifacts and mixed-backend
  comparisons use bounded object traversal. Historical diffs require the saved
  objects.
- Branch and tag discovery reads at most 512 advertised refs within 256 KiB.
  Search is scoped to one Git snapshot. Commit search follows up to 24
  first-parent commits. Code search skips non-text files and files over 64 KiB
  and reports when the scan or result budgets are reached.
- Source downloads contain regular files and directories up to 16 MiB and 2,000
  entries in `ustar` format. Use a Git mirror for history, symlinks and submodules.
- The edge limit is 300 requests per minute per source and traffic class, counted
  separately at each Cloudflare location. [Rate Limiting binding](https://developers.cloudflare.com/workers/runtime-apis/bindings/rate-limit/)
- Historical issue closures from before state history was introduced appear without
  an actor.

## Layout

- Worker routes, permissions, and Git backends live in [src/](src/).
- Server-rendered HTML and styles live in [templates/](templates/) and [static/](static/).
- D1 schema changes live in [migrations/](migrations/).
- The build bootstrap and native Git origin live in [scripts/](scripts/).
- Isolated HTTP and Git scenarios live in [tests/](tests/).

## Design

Shard follows the system's light or dark appearance. Geist and Shard Han set
interface text with tabular figures. JetBrains Mono sets code, paths and Git
identifiers. Askama renders the pages. Repository tabs wrap on narrow screens.
Supporting browsers animate same-origin page changes, menus and native pickers
according to the system's reduced-motion preference.
[Design guide](DESIGN.md) · [Palette](static/css/theme.css) · [Stylesheet](static/css/shard.css)

## Prior art

- [ripgit](https://github.com/deathbyknowledge/ripgit) — one Durable Object per repo,
  SQLite objects, FTS5, and text/markdown negotiation for agents.
- [git-on-cloudflare](https://github.com/zllovesuki/git-on-cloudflare) — Smart HTTP
  v2 with Durable Object refs, R2 packs and a streaming push pipeline.
- [durable-git](https://github.com/littledivy/durable-git) — Git implemented inside a
  Durable Object, optional R2 pack cache, cgit-style surface.

## Cloudflare API notes (verified 2026-11-17)

- `wrangler@4.164.0`'s schema accepts `artifacts: [{ binding, namespace, remote? }]`.
  The local simulator requires Wrangler ≥ 4.163 and Git ≥ 2.32 on `PATH`.
- Workers binding surface: namespace `create/get/list/import/delete`; repo handle
  `info/createToken/listTokens/revokeToken/fork/log/readCommit/readTree/readBlob/readFile`.
- Shard uses Git HTTPS for ref discovery and supported ref writes. The optional
  native origin stages Artifacts merges that require a pack with new objects.
- Commit metadata uses `treeHash` and `authoredAt`/`committedAt` (Unix seconds);
  tree entries are `{ name, mode, type, hash }`.
- Git remotes accept a Bearer repo token; for Basic auth the password is the token
  secret before its `?expires=` suffix.

[cloudflare-deploy]: https://deploy.workers.cloudflare.com/?url=https%3A%2F%2Fgithub.com%2Faliveranme%2Fshard
