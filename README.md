# Codex Provider Hub

macOS 菜单栏（托盘）看板：接管本机 [Sub2API](https://github.com/Wei-Shaw/sub2api) 多供应商 Codex 网关，并实时展示用量 / 额度。

## 功能

- **本地网关** — 起停 / 健康检查（默认 `127.0.0.1:18080`），供应商与模型数量
- **Sub2API 号池** — 5 小时 / 7 天剩余额度、池内可用账号数
- **AIHub 中转站** — 钱包余额与今日消耗（读 `ANYROUTER_API_KEY`）
- **Cursor 多账号** — 导入本机会话或粘贴 JWT，逐账号显示套餐用量（token 本地加密存储）

## 技术栈

- Tauri v2 + Rust
- React + TypeScript + Vite

## 环境要求

- macOS（推荐 Apple Silicon）
- [Node.js](https://nodejs.org/) 20+
- [Rust](https://rustup.rs/) stable
- 可用的 Sub2API 部署（Docker Compose），并带有 `./sub2api` 管理脚本

## 配置说明

| 数据源 | 凭据 / 路径怎么找 |
|--------|-------------------|
| Sub2API 安装目录 | 环境变量 `SUB2API_DIR` 或 `CODEX_PROVIDER_HUB_SUB2API_DIR`；否则默认 `$HOME/Documents/Codex/sub2api-ready` |
| 网关 API Key | `$SUB2API_DIR/state/gateway-api-key` |
| AIHub Key | 环境变量 `ANYROUTER_API_KEY`，或在 `~/.zshrc` 里 `export ANYROUTER_API_KEY=...` |
| Codex 配置（可选保存） | `~/.codex/config.toml` + model catalog JSON |
| Cursor Token | 加密保存在应用数据目录；也可从 Cursor 的 `state.vscdb` 一键导入 |

示例：

```bash
export SUB2API_DIR="$HOME/path/to/your/sub2api-ready"
export ANYROUTER_API_KEY="sk-..."
```

## 开发

```bash
npm install
source "$HOME/.cargo/env"   # 如有需要
npm run tauri dev
```

点击菜单栏图标显示 / 隐藏看板。关闭窗口会隐藏到托盘（无 Dock 图标）。

## 构建

```bash
npm run tauri build
```

产物路径：

- `src-tauri/target/release/bundle/macos/Codex Provider Hub.app`
- `src-tauri/target/release/bundle/dmg/*_aarch64.dmg`

## 安全说明

- API Key **不会**写死在源码里，运行时从环境变量 / 本地文件读取
- Cursor access token 使用 AES-256-GCM 加密存储（密钥由机器标识 + 应用 salt 派生）
- 保存供应商配置前会给 `config.toml` / catalog 打带时间戳的备份，且**不会**改写 `model_provider` id（Codex 会话列表按该 id 过滤）

## License

MIT
