# AGENTS.md — 0721 仓侧规则

> 完整工作区不变量 = `/mnt/codespace/AGENTS.md`（每轮注入）；本文件只放本仓特有事实。
> 冲突时：用户即时指令 > 工作区 AGENTS > 本文件。改造计划唯一家 =
> `/mnt/codespace/_reports/0721-refactor-20261122/PLAN.md`。

## 本仓是什么

- 网盘 + 图床资源服务（langyo 个人仓，Apache-2.0）；staging 上线 `res.celestia.world`
  （node-1:3026，`i0721-staging.service`）；OAuth 对接 `auth.celestia.world`（ERP provider
  `res-celestia`），开源版支持 Google/GitHub/飞书等 provider + 可选账密双模式。
- 0721 是 rustfs 的独立前端（自有桶 + 桶级凭据），与 arona `storage.*`（平台服务面 + 开通面）并列、互不隶属；chest / entelecheia 的静态资源读写一律经 arona，**禁止**把它们接到 0721。目标模型唯一家 = `_reports/media-hosting-consolidation-20261124/REPORT.md`（工作区路径）。

## 架构终态（11 波全部合并，#19–#26）

- **服务端**：axum 0.8 + tokio + lacrymira ^0.2（PostgreSQL，`i0721-pg`）+ jiff（禁 chrono）。
- **通信面**：WS JSON-RPC（plana-rpc-server @ `/api/rpc`，一次性 WS ticket + httpOnly cookie）+
  REST 白名单（health / auth bootstrap / refresh / logout / ws-ticket / upload / media / share / MCP SSE+message）+
  MCP（SSE transport + 5 agent 工具 + API key `i0721-<32hex>` 哈希存储 + copy-once +
  每 key 权限 `read`/`upload`/`share` 与可选过期，逐工具执行）。
- **数据模型**：users（`quota_bytes`）/ `oauth_identities` / files（folder + `parent_id` + 同层唯一索引 +
  软删 + 级联回收）/ shares / `api_keys` / settings 六表。
- **存储**：`I0721_STORAGE_BACKEND` = local | s3（`object_store` ^0.12）| ipfs（kubo RPC + MFS）。
- **webui**：hikari Vue/TS，**家族范式对齐**（2026-11-23 webui 波次；参照 ayane 的迭代）：
  `HkAdminShell` 四槽壳（header/sidebar/content/footer）+ 分组侧边栏（内容/接入/管理，管理组 admin 专属）
  + `HkAdminHeader` 身份面（头像账户窗/语言级联/登出）+ `HkStatusBar` 页脚（面板版本+引擎身份+传输探针）
  + `HkLoginFlow` 登录面（账密通道 + provider sso 通道，单通道无段组）+ `HkAdminTablePage` 四张名册（列契约 = `HTableColumn` 类型） + `HkMasterDetail` 三面主从（Files/Portal/Timeline）+ `HkSelect`/`HkRadio`/`HkSwitch` 表单族
  + `HkAboutBody` 关于页 + `HkToast` 根挂载 + 12 语种 i18n（零未用键）+ ts-rs 单向类型。
  **共享工厂**（勿再逐视图拷贝）：`composables/errorText.ts`（never-empty 失败文案）、
  `composables/formatSize.ts`（二进制单位尺寸）。
- **会话**：httpOnly cookie（SameSite=Lax + Secure 按请求 X-Forwarded-Proto 判定 +
  `I0721_COOKIE_SECURE=auto|always|never` 钉）；WebSocket 用一次性 ticket；MCP 用 API key。

## 开发纪律

- trunk = `master`，只经 PR + squash merge；标题 = `<gitmoji> 一句英文陈述句.`
  （钩子由 `celestia-devtools init` 安装）。
- 门禁 = `just gate`（webui build → fmt → clippy -D warnings → test → deny → vitest → barrel →
  lockfile → i18n）；PG 车道 = `just pg-test`（序列化，索引重建测试不能并行）；
  devtools 漂移检查 = `celestia-devtools init --check`。
- CI 口径 = trunk 基本检查（push master：<1 分钟——verify-versions + depspec-lint + family-versions）+
  `ci.yml` 手动 dispatch（全量门禁）。
- 依赖声明两段式（`^0.8` / `^0.1` 形）；**不直接 use sqlx**（逃生舱走 lacrymira `Command::Sql`）。
- 右上角动作区语法（主创建动作 = 动作区加号，槽位唯一家 = hikari `docs/<locale>/design/page-chrome.md`）；
  i18n = JSON `res/i18n/locales/<lang>/<namespace>.json`，en + zh-Hans baseline，`check_i18n` 硬门禁。
- 版本：树内 0.4.0；**发布 tag 挂起**至用户目验裁决（`publish.yml` 由 `v*` tag 触发）。

## 部署（staging 实测）

- 单元 `i0721-staging.service`（node-1:3026）；部署根 `/srv/celestia/i0721-staging/`；
  引导根 `/mnt/codespace/_secure/i0721-staging.env`（600）。
- daemon vhost `res-celestia-world` → node-1:3026（SSE 车道关缓冲/3600s）。
- ERP client `res-celestia` 已注册于 node-3 `erp-personal/oauth-clients.env`。
- 公网隧道（SakuraFrp）待用户面板创建（A 组 = auth+admin+res .celestia.world）。

## 遗留（后续波次）

- DashMap 替代 Mutex 注册表；Range/流式读取；
  per-file 存储放置；MFS 多节点映射入库；text 预览大小帽；上传取消/拖拽。
- `cred_scan` 测试夹具豁免已登记 `_ledger/tool-exemptions.md`。
