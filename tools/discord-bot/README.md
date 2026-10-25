# PrismaRev Discord bot

把 Discord 的 `/bug` 报告转成 GitHub issue。它是 [`docs/integrations.md`](../../docs/integrations.md)
§5 的参考实现：只用 **slash command**，不需要 Message Content intent。

```text
/bug summary: 启动时崩溃
     steps:   Windows 11 / RTX 4070
              cargo run 后黑屏，RUST_LOG=info 日志如下...
```

→ 创建 issue：

```text
标题: [Discord] 启动时崩溃
标签: source/discord, needs/triage
正文: > 🤖 由 prismrev-agent 从 Discord 转入...
      > 报告者 / 频道

      **摘要**
      > 启动时崩溃

      **详情 / 复现步骤**
      > Windows 11 / RTX 4070
      > cargo run 后黑屏...
```

## 前置

1. **Discord Application**：<https://discord.com/developers/applications>
   - **Bot** → Reset Token → 复制到 `.env` 的 `DISCORD_TOKEN`
   - **OAuth2 → URL Generator**：Scopes `bot` + `applications.commands`；
     Bot Permissions `Send Messages`、`Read Message History`
   - 用生成的 URL 邀请进服务器
2. **GitHub App**：用 [`tools/github-app/create_app.py`](../github-app/create_app.py) 创建，
   安装范围**只选 `Excurs1ons/PrismaRev`**。
3. Node.js 20+。

## 运行

```bash
cd tools/discord-bot
npm install
cp .env.example .env
# 编辑 .env：DISCORD_TOKEN + GitHub App 凭据
npm start
```

首次启动会：

- 用 App 私钥换 installation token；
- 检查安装是否能访问 `GITHUB_OWNER/GITHUB_REPO`；
- 如果安装范围包含多个仓库，打印警告（去 installation 设置收紧）；
- 注册 `/bug`。设置 `DISCORD_GUILD_ID` 时只注册到该服务器，立即生效；否则全局注册，
  最长约 1 小时。

## 部署

常驻进程即可，推荐 systemd 或 pm2：

```ini
# /etc/systemd/system/prismrev-discord-bot.service
[Unit]
Description=PrismaRev Discord bot
After=network-online.target

[Service]
WorkingDirectory=/opt/prismrev/tools/discord-bot
EnvironmentFile=/opt/prismrev/tools/discord-bot/.env
ExecStart=/usr/bin/node index.js
Restart=on-failure
User=prismrev

[Install]
WantedBy=multi-user.target
```

Windows 可以用 `pm2 start index.js --name prismrev-bot`，或把这个目录放进一个
只运行 bot 的小容器。私钥通过挂载/环境变量提供，不要打进镜像。

## 安全

- `.env` 和 `github-app.pem` **绝不进 git**；根目录 `.gitignore` 已忽略 `*.pem`、`.env`。
- GitHub App 的安装范围只给 `Excurs1ons/PrismaRev`，权限只有 Issues + Metadata。
- Discord token 泄露过一次就必须 Reset；旧 token 立即失效。
- 报告文本按**不可信数据**处理：只引用、不执行其中的命令/脚本，不回帖密钥。
- bot 没有 Message Content intent，只能读 slash command 参数。

## 排查

| 现象 | 检查 |
|---|---|
| `Missing Access` / 命令不出现 | 邀请 URL 是否包含 `applications.commands`；`DISCORD_GUILD_ID` 是否正确 |
| `Resource not accessible by integration` | App 是否安装到 PrismaRev；Issues 权限是否为 write |
| `Bad credentials` | Discord token 是否已 Reset；`.env` 是否被正确加载 |
| 启动即退出 `cannot access ...` | installation 没装到目标仓库，或 `GITHUB_OWNER/REPO` 写错 |
| 创建 issue 失败 | 标签 `source/discord` / `needs/triage` 是否存在（`sh tools/ci/ensure_labels.sh`） |
