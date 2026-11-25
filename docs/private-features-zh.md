# CC Switch 私有功能说明

本文记录私有分支相对官方版新增或强化的功能。当前版本基线为官方 **v4.0.40**，私有版本号形如
`4.0.40-kimicode.<时间戳>`，开发分支 `feat/usage-sync-deepseek`。

## 1. Kimi Code 全链路接入

- 新增 Kimi Code 应用（应用 ID `kimi`，数据目录 `~/.kimi-code`），支持原生
  `config.toml` 供应商与模型管理，保留 TOML 注释和未修改字段。
- 供应商预设复用 Pi 目录并转换为 Kimi 原生协议：`kimi`、`openai`、
  `openai_responses`、`anthropic`、`google-genai`、`vertexai`。
- 支持直连、路由、聚合三种模式：Kimi 已纳入 `route_mode` 应用，具备本地代理契约、
  原生 headers 透传和 Codex 推理档位选择。
- OAuth 授权中心支持多个 Kimi 账号：设备码登录、国内/海外区域、昵称、额度、重新登录、
  删除和「使用此账号」；凭据按官方格式保存在 `~/.kimi-code/credentials/`。
- 支持 MCP（`mcp.json`，含 SSE）、Skills、全局提示词（`AGENTS.md`）以及模型能力配置。
- 详细说明见 `docs/kimi-code-integration-zh.md`。

## 2. Kimi 会话管理与恢复

- Kimi 会话列表、搜索、阅读、删除、重命名；阅读器只展示活动分支，避免执行记录重复。
- 旧模型下线或模型别名被移除时，会话「更多」中提供 **换模型继续**：选择当前配置里的
  可用模型，保留原会话 ID 和工作目录，尝试原地恢复。
- 无法原地恢复时，可在阅读页预览并复制最近的提问和最终回复到新会话；不改写历史记录，
  也不会自动发送。
- 相关代码：`src-tauri/src/session_manager/providers/kimi/`、
  `src/components/sessions/KimiResumeDialog.tsx`、
  `src/components/sessions/reader/kimiContinuation.ts`。

## 3. 用量统计与计价

- Kimi 用量导入：兼容旧格式 assistant 消息和新格式 `step.end`，用 `usage.record` 补全模型；
  持久去重账本避免日志重写、会话分叉或明细清理后重复计费。
- 计时补齐：优先使用 `step.end` 原生首字/流式时长；旧日志用 `step.begin`、首个
  `content.part` 等时间戳估算，界面以 `≈` 标注。速度按输出 token ÷ 总耗时显示，
  输出不少于 200 token、耗时不少于 1 秒才估算。
- 金额展示支持人民币；DeepSeek 供应商卡片显示当日花费，而不是余额。
- DeepSeek 官方计价：
  - 补齐官方模型 ID（如 `deepseek-v4.1-flash-1017`）和版本后缀回落；
  - 按北京时间工作日 09:00–12:00、14:00–18:00 计高峰价，其余时段、周末和法定节假日半价；
  - 启动时一次性重算历史 DeepSeek V4 成本。
- 相关代码：`src-tauri/src/services/session_usage_kimicode.rs`、
  `src-tauri/src/services/deepseek_pricing.rs`。

## 4. 多终端用量汇总

- 复用现有 S3/OSS 云同步配置，每台设备只上传**用量快照**：本机明细 + 本机日汇总 +
  设备标识；不上传配置、密钥或提示词。
- 其他设备自动拉取并合并到本机 `proxy_request_logs` / `usage_daily_rollups`；
  对端明细 request_id 统一加 `peer:<device>:` 前缀，并用持久回执防止重复导入。
- 本机日汇总只统计本机行，对端汇总带设备维度单独存放，避免多设备互相累计造成重复计费。
- 请求列表对来自其他终端的记录显示设备徽标；用量设置中有「多终端汇总」面板，可开关、
  查看设备列表、立即汇总或移除本机副本。启用后默认每 15 分钟自动同步。
- 相关代码：`src-tauri/src/services/usage_sync.rs`、
  `src/components/usage/UsageSyncSection.tsx`、数据库 v29 迁移。

## 5. OSS / S3 数据备份

- 数据页支持 SQL 导入/导出，以及私有 OSS SQL 备份：导出当前数据库并上传、列出远端备份、
  下载并导入，便于 A 主机上传、B 主机恢复。
- 备份对象使用私有 ACL；支持 OSS 虚拟主机风格、`.aliyuncs.com.cn`、内网和加速 Endpoint，
  对不匹配的 Endpoint 收紧校验和错误提示。
- SQL 备份包含数据库内的供应商配置和密钥，因此只保存为私有对象。
- 相关代码：`src-tauri/src/services/oss_backup.rs`、`src-tauri/src/services/s3.rs`。

## 6. 私有发布与更新通道

- 版本号追加 `-kimicode.<时间戳>`，与官方版本区分；应用内自动更新保持关闭。
- 「关于」页显示独立分支状态，并提供手动「检查更新」，从私有 OSS 清单
  `cc-switch-updates/latest.json` 读取新版本。
- 独立更新签名密钥；构建脚本生成 DMG、`.app.tar.gz` 和 `.sig`，发布脚本上传 OSS 并更新清单。
- 发布流程：
  - `pnpm build:branch`：写入私有版本号，构建并生成 DMG/更新包；
  - `pnpm publish:update --prefix cc-switch-updates --dmg <DMG路径> --notes <说明>`：
    上传产物并更新 `latest.json`。
- 安装包目前为 ad-hoc 签名，未做 Apple 公证；首次打开如被拦截需在系统设置中允许。
- 相关代码：`scripts/build-branch.mjs`、`scripts/publish-update.mjs`、
  `src/components/settings/AboutSection.tsx`。

## 7. 官方合并与验证

- 已合并官方 v4.0.38、v4.0.40，并在合并中保留 Kimi 私有路由、用量账本、OSS 和多终端能力。
- 数据库当前 schema v29；包含多终端来源设备维度、日汇总设备主键和 DeepSeek 重算标记。
- 最近验证结果：
  - 前端 4,828 项测试通过；
  - Rust 5,258 项单元测试 + 232 项集成测试通过；
  - TypeScript、Prettier、Knip、Clippy `-D warnings` 通过；
  - DMG 本地验签、隔离 HOME 启动冒烟、OSS 重新下载验签通过。
- 真实数据库副本验证：4,792 条 DeepSeek 0 成本记录按官方峰谷价重算为 $32.73。

## 边界与限制

- 多终端汇总仅支持 S3/OSS；各设备都需要升级到私有版本并显式开启。WebDAV 暂不复用该协议。
- 各设备按本机时区生成日汇总日期；跨时区设备暂未做日期桶对齐。
- 对端历史用量在明细保留期内可查看请求详情；更早数据只有日/服务商/模型汇总，首字中位数等
  明细指标不回溯。
- DeepSeek 官方价为 USD，人民币金额仍按设置中的手动汇率换算。
- 中国法定节假日表先维护 2026–2027，之后需要按年更新。
- 自动更新关闭；新版本通过「关于 → 检查更新」手动安装。