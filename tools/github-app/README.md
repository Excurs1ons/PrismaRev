# GitHub App setup（manifest flow）

GitHub **没有** “创建 App”的 REST API。唯一的自动化路径是官方
[App Manifest flow](https://docs.github.com/en/apps/sharing-github-apps/registering-a-github-app-from-a-manifest)：
浏览器里确认一次，GitHub 回调一个一次性 code，脚本用它换 App ID、私钥和 client secret。

`create_app.py` 把除“点按钮”以外的步骤都做掉：

1. 起一个本地回调服务器；
2. 生成 manifest 并打开 GitHub 创建页；
3. 你点 **Create GitHub App**；
4. 脚本换取凭据，写到仓库外，并打印安装链接；
5. 你点 **Install**，脚本记录 `installation_id`。

2FA 和账号密码始终在你那边，脚本不接触。

## 用法

```bash
# 个人账号，默认 App 名 prismrev-agent，凭据写到 ~/.prismrev/github-app
python tools/github-app/create_app.py

# 组织账号
python tools/github-app/create_app.py --owner my-org --owner-type org --name prismrev-agent

# 只看 manifest URL，不启动服务器
python tools/github-app/create_app.py --dry-run
```

默认回调地址是 `http://localhost:<随机端口>/callback`，不需要公网。
如果 900 秒内没完成，脚本退出；已创建的 App 和凭据仍保留，重新运行会创建新的 App。

## 创建的权限

- Repository permissions: **Issues: Read and write**、**Metadata: Read-only**
- 不订阅 webhook（`default_events: []`）
- Private App，只安装到 `Excurs1ons/PrismaRev`

安装时在 GitHub 的安装页**只勾选 PrismaRev**，不要选 “All repositories”。

## 凭据位置

默认在仓库外：

```text
%USERPROFILE%\.prismrev\github-app\
├── github-app.pem      # App 私钥（0600）
├── github-app.json     # app_id / slug / client secrets / installation_id
└── create-url.txt      # 本次 manifest URL（可删）
```

`github-app.json` 含 `client_secret` / `webhook_secret`，同样是敏感文件。
**不要把 `.pem` 或这个 JSON 提交进 git**；仓库已忽略 `*.pem`、`.secrets/`、`.env`。

## 给 bot 的环境变量

```bash
GITHUB_APP_ID=<github-app.json 里的 app_id>
GITHUB_APP_PRIVATE_KEY_PATH=<github-app.pem 的绝对路径>
GITHUB_INSTALLATION_ID=<安装后记录的值；不填则由 bot 列出 installations>
```

Bot 用 App JWT 换 1 小时有效的 installation token；不要用私钥直接调 REST。
如果 `installation_id` 没记录到，bot 可以先 `GET /app/installations` 找到对应仓库的安装。

## 轮换 / 吊销

- 私钥泄露：GitHub App 设置 → **Private keys** → 删除旧 key → **Generate a private key**，更新
  `GITHUB_APP_PRIVATE_KEY_PATH` 指向的新文件。
- 不想用了：App 设置 → **Delete GitHub App**；安装会自动失效。
- 重新跑脚本会创建**新 App**，不会覆盖旧 App。

## 为什么不能全自动

GitHub 要求 App 的创建和安装由登录用户在浏览器里确认，这是防止第三方静默创建
OAuth/App 权限的安全边界。脚本能自动化的是 manifest、回调、换取凭据和安装 ID。
