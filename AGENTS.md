# AGENTS.md — 0721 仓侧规则

> 完整工作区不变量 = `/mnt/codespace/AGENTS.md`（每轮注入）；本文件只放本仓特有事实。
> 冲突时：用户即时指令 > 工作区 AGENTS > 本文件。改造计划唯一家 =
> `/mnt/codespace/_reports/0721-refactor-20261122/PLAN.md`。

## 本仓是什么

- 图床 → 网盘 + 图床资源服务（langyo 个人仓，Apache-2.0）；部署候选 `res.celestia.world`，对接
  `auth.celestia.world`（开源版支持 Google/GitHub/飞书等 provider + 可选账密）。
- 重构波次：axum 0.8 + lacrymira（元数据 PostgreSQL）+ hikari Vue/TS webui + 本地/S3/IPFS 存储 +
  MCP（API key，SSE + Bearer）。旧栈（Yew / sea-orm / tairitsu / Cloudflare Workers 双态）随波退役：
  **Yew client + SSR 页已在 bootstrap 波先行退役**（上游 `hikari-boot` 已消失，车道本就不可构建；
  过渡期首页为占位页，媒体与 API 照常）。

## 开发纪律

- trunk = `master`，只经 PR + squash merge；标题 = `<gitmoji> 一句英文陈述句.`
  （钩子由 `celestia-devtools init` 安装；CI 口径 = trunk 基本检查 <1 分钟 + `ci.yml` 手动 dispatch）。
- 门禁 = `just gate`（fmt → clippy -D warnings → test → cargo deny）；
  devtools 漂移检查 = `celestia-devtools init --check`。
- 依赖声明两段式（`^0.8` / `^0.1` 形）；时间一律 jiff 禁 chrono；不直接 use sqlx
  （逃生舱走 lacrymira `Command::Sql`）。`wasm-bindgen` 精确钉版是 wasm-bindgen-cli 配套的
  过渡豁免，Yew 退役波一并移除。
- webui 波落地后：右上角动作区语法（主创建动作 = 动作区加号，槽位唯一家 = hikari
  `docs/<locale>/design/page-chrome.md` + `useActionBar` 注释）；i18n = JSON
  `res/i18n/locales/<lang>/<namespace>.json`，en + zh-Hans baseline，`check_i18n` 硬门禁。
- 版本：树内版本随发版波统一升 0.4.0；**发布 tag 挂起**至用户目验裁决（`publish.yml` 由
  `v*` tag 触发，不打 tag 即不发版）。
