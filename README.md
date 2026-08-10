# Codex Provider Hub

macOS 菜单栏（托盘）看板：接管本机 [Sub2API](https://github.com/Wei-Shaw/sub2api) 多供应商 Codex 网关，并实时展示用量 / 额度。

## 功能

- **本地网关** — 起停 / 健康检查（默认 `127.0.0.1:18080`），供应商与模型数量
- **Codex 模型选择器守护** — 修复 ChatGPT.app 因 Statsig `use_hidden_models=true` 滤空自定义模型的问题；一键补丁 Local Storage，并以 `--host-rules` 防刷新方式重启 ChatGPT；默认后台巡检
- **供应商** — 输入上游 Base URL + API Key，接入本地 Sub2API，并同步到 Codex model catalog
- **Sub2API 号池** — 仅统计 OpenAI/Codex **OAuth** 真号；每账号独立 5h/7d 额度卡，支持删除；中转站不计入
- **AIHub 中转站** — 钱包余额与今日消耗（优先读 Sub2API 里 AIHub 账号 key，可在卡内设置/清除 Key）
- **Cursor 多账号** — 导入本机会话或粘贴 JWT，逐账号显示套餐用量（token 本地加密存储）
- **托盘交互** — 点击菜单栏图标，看板贴在图标下方弹出（非屏幕居中）

UI 为深色电影感控制台风格（参考 [CineFlux](https://cine-flux.com) 的气质，非抄袭品牌资产）。

## 技术栈

- Tauri v2 + Rust
- React + TypeScript + Vite

## 环境要求

- macOS（推荐 Apple Silicon）
- [Node.js](https://nodejs.org/) 20+
- [Rust](https://rustup.rs/) stable
- 可用的 Sub2API 部署（Docker Compose），并带有 `./sub2api` 管理脚本
- 模型选择器守护推荐本机 Python3 + `plyvel`（Hub 会优先走捆绑的 patch 脚本；无 plyvel 时回退字节替换）

## 配置说明

| 数据源 | 凭据 / 路径怎么找 |
|--------|-------------------|
| Sub2API 安装目录 | 环境变量 `SUB2API_DIR` 或 `CODEX_PROVIDER_HUB_SUB2API_DIR`；否则默认 `$HOME/Documents/Codex/sub2api-ready` |
| 网关 API Key | `$SUB2API_DIR/state/gateway-api-key` |
| Sub2API Admin | `$SUB2API_DIR/.env` 中的 `ADMIN_EMAIL` / `ADMIN_PASSWORD`（供应商 / 号池管理） |
| AIHub Key | 优先 Sub2API `AIHub` 账号；其次 Hub 内「设置 Key」；再次 `ANYROUTER_API_KEY` / `~/.zshrc`（注意不要把 AnyRouter key 当成 AIHub key） |
| Codex 配置（可选保存） | `~/.codex/config.toml` + model catalog JSON |
| Cursor Token | 加密保存在应用数据目录；也可从 Cursor 的 `state.vscdb` 一键导入 |

示例：

```bash
export SUB2API_DIR="$HOME/path/to/your/sub2api-ready"
```

## 如何添加供应商

1. 确保本地网关已启动（看板「本地网关」为 healthy，或在 `$SUB2API_DIR` 执行 `./sub2api up`）。
2. 打开看板中的 **供应商** 卡片 → **添加**。
3. 填写显示名、Base URL（如 `https://xxx.example.com/v1`）、API Key、模型前缀。
4. 可先 **仅探测**，再 **添加供应商**（勾选「添加时探测并同步模型」）。
5. Hub 会创建 Sub2API `apikey` 账号、备份并更新 Codex catalog（**不会**改 `model_provider` id）。
6. 在 Codex 中选择 `{prefix}-{model}` 即可（流量仍走 `http://127.0.0.1:18080/v1`）。

本机若 `SECURITY_URL_ALLOWLIST_ENABLED=false`，一般无需改 compose。若重新打开白名单后又出现 `502 host not allowed`，写入 `SECURITY_URL_ALLOWLIST_UPSTREAM_HOSTS` 后需对容器 **force-recreate**。

## Codex 模型选择器守护

ChatGPT 桌面端会用 Statsig 动态配置过滤非官方 slug。守护卡可：

1. 将 Local Storage 中 `use_hidden_models` 置为 `false`
2. 以屏蔽 Statsig CDN 的方式重启 ChatGPT（`--host-rules`）

**请勿从 Dock 裸启动 ChatGPT**，否则 Statsig 可能再次覆盖。可用 Hub 内「立即修复并防刷新启动」，或 `~/Applications/ChatGPT (Guarded).command`（若已生成）。

## 开发

```bash
npm install
source "$HOME/.cargo/env"   # 如有需要
npm run tauri dev
```

## 构建

```bash
npm run tauri build
```

产物：

- `src-tauri/target/release/bundle/macos/Codex Provider Hub.app`
- `src-tauri/target/release/bundle/dmg/*_aarch64.dmg`

## 安全说明

- API Key **不会**写死在源码里，运行时从环境变量 / 本地文件 / 加密 app-data 读取
- Cursor / AIHub 自定义 Key 使用 AES-256-GCM 加密存储
- 保存供应商配置前会备份 `config.toml` / catalog，且不改写 `model_provider` id

## License

MIT
