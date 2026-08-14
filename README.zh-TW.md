# xURL

[English](README.md) | **繁體中文**

`xURL` 是一套 CLI 工具，透過統一的 `agents://` URI scheme 讀取、查詢與寫入 AI agent 對話。

> 又稱 **Xuanwo's URL**。

## 安裝此 Fork

此 fork 新增了上游版本未包含的 `agents://agy` provider，可用來讀取 [Google Antigravity CLI](https://antigravity.google) 的對話。

透過 Cargo 直接從 GitHub 安裝 CLI：

```bash
cargo install --git https://github.com/zeta987/xurl xurl-cli --tag v0.0.28 --force
```

安裝此 fork 的 agent skill：

```bash
npx skills@latest add zeta987/xurl -g -y -s xurl
```

安裝完成後的指令為 `xurl`。加上 `--force` 可讓 Cargo 覆蓋既有的 `xurl`；若想追蹤 `main` 分支上的最新 commit 而非特定版本，請移除 `--tag v0.0.28`。

這兩道指令都會就地覆蓋既有的上游安裝，因此每一台需要 `agy` provider 的電腦都要各跑一次。

## xURL 能做什麼

xURL 提供單一 URI scheme（`agents://`），讓你能跨多個 AI agent CLI **讀取**、**查詢**、**探索**與**寫入**對話。

- 以 markdown 格式**讀取**對話 — `xurl agents://codex/<id>`
- 依 provider、關鍵字、本機路徑或角色**查詢** thread — `xurl 'agents://codex?q=refactor'`
- 深入檢視前先**探索**子目標與 metadata — `xurl -I agents://codex/<id>`
- **寫入**以開始或繼續對話 — `xurl agents://codex -d "hello"`

## Providers

<table>
  <tr>
    <td align="center"><img src="https://cdn.simpleicons.org/googlegemini" alt="Antigravity" width="36" height="36" /><br /><code>agents://agy</code></td>
    <td align="center"><img src="https://ampcode.com/amp-mark-color.svg" alt="Amp" width="36" height="36" /><br /><code>agents://amp</code></td>
    <td align="center"><img src="https://cdn.simpleicons.org/claude" alt="Claude" width="36" height="36" /><br /><code>agents://claude</code></td>
    <td align="center"><img src="https://avatars.githubusercontent.com/u/14957082?s=200&v=4" alt="Codex" width="36" height="36" /><br /><code>agents://codex</code></td>
    <td align="center"><img src="https://cdn.simpleicons.org/githubcopilot" alt="GitHub Copilot" width="36" height="36" /><br /><code>agents://copilot</code></td>
  </tr>
  <tr>
    <td align="center"><img src="https://www.cursor.com/favicon.ico" alt="Cursor" width="36" height="36" /><br /><code>agents://cursor</code></td>
    <td align="center"><img src="https://cdn.simpleicons.org/googlegemini" alt="Gemini" width="36" height="36" /><br /><code>agents://gemini</code></td>
    <td align="center"><img src="https://avatars.githubusercontent.com/u/129152888?s=200&v=4" alt="Kimi" width="36" height="36" /><br /><code>agents://kimi</code></td>
    <td align="center"><img src="https://avatars.githubusercontent.com/u/208539476?s=200&v=4" alt="OpenCode" width="36" height="36" /><br /><code>agents://opencode</code></td>
    <td align="center"><img src=".github/assets/pi-logo-dark.svg" alt="Pi" width="36" height="36" /><br /><code>agents://pi</code></td>
  </tr>
</table>

## 上游套件

以下管道安裝的是上游版本，不含 `agy` provider，僅在無法使用 Rust 時才需要。

安裝為 agent skill：

```bash
npx skills add Xuanwo/xurl
```

或安裝為獨立 CLI：

```bash
brew tap xuanwo/tap && brew install xurl   # Homebrew
cargo install xurl-cli                      # Cargo
uv tool install xuanwo-xurl                 # Python / uv
npm install -g @xuanwo/xurl                 # npm
```

## 快速開始

讓你的 agent 摘要某個 thread：

```text
Please summarize this thread: agents://codex/xxx_thread
```

## 使用方式

> **注意：** `agents://` scheme 前綴可省略 — `codex/...` 等同於 `agents://codex/...`。

### 讀取

```bash
xurl agents://codex/019c871c-b1f9-7f60-9c4f-87ed09f13592
xurl agents://copilot/688628a1-407a-4b4e-b24a-1a250ebf864f
```

將輸出儲存至檔案：

```bash
xurl -o /tmp/conversation.md agents://codex/019c871c-b1f9-7f60-9c4f-87ed09f13592
```

### 查詢

依 provider 查詢：

```bash
xurl agents://codex
xurl 'agents://codex?q=spawn_agent'
xurl 'agents://claude?q=agent&limit=5'
xurl 'agents://copilot?q=resume&limit=5'
```

依本機路徑查詢：

```bash
xurl agents:///Users/alice/work/xurl
xurl 'agents:///Users/alice/work/xurl?q=refactor&limit=5'
xurl 'agents://.?q=refactor&providers=codex,claude'
xurl 'agents://~/work/xurl?providers=opencode'
```

依角色查詢：

```bash
xurl agents://codex/reviewer
```

查詢結果若有資料，會附上精簡的 thread metadata，讓你不必逐一開啟每個 thread 就能檢視 `payload.git.branch` 這類欄位。

### 探索

```bash
xurl -I agents://codex/019c871c-b1f9-7f60-9c4f-87ed09f13592
```

Frontmatter 會將 provider metadata 攤平成易讀的鍵值行（例如 `payload.git.branch = ...`），並自動略過過長的指令欄位。

深入檢視探索到的子目標：

```bash
xurl agents://codex/019c871c-b1f9-7f60-9c4f-87ed09f13592/019c87fb-38b9-7843-92b1-832f02598495
```

### 寫入

開始新對話：

```bash
xurl agents://codex -d "Draft a migration plan"
```

以角色 URI 開始：

```bash
xurl agents://codex/reviewer -d "Review this patch"
xurl agents://copilot/research -d "Investigate the failing integration test"
```

繼續既有對話：

```bash
xurl agents://codex/019c871c-b1f9-7f60-9c4f-87ed09f13592 -d "Continue"
```

透過 query string 傳遞額外參數給 provider CLI：

```bash
xurl "agents://codex?cd=%2FUsers%2Falice%2Frepo&add-dir=%2FUsers%2Falice%2Fshared&model=gpt-5" -d "Review this patch"
```

## 指令參考

```bash
xurl [OPTIONS] <URI>
```

- `-I, --head`：只輸出 frontmatter 與探索資訊；若有 provider metadata，會將第一筆記錄攤平成鍵值行一併輸出。
- `-d, --data <DATA>`：寫入 payload（可重複指定）。
  - 文字：`-d "hello"`
  - 檔案：`-d @prompt.txt`
  - stdin：`-d @-`
- `-o, --output <PATH>`：將指令輸出寫入檔案。

## 錯誤輸出

`xurl` 會向 agent 輸出具操作指引的 stderr 錯誤訊息：

- 遇到不支援的 provider 或功能時，會附上 `requested_uri`、建議的 `next_steps` 以及可回報支援需求的 GitHub issue 連結
- 找不到本機資料時會附上 `searched_roots` 等路徑資訊，讓下一步的排查方向更明確
- provider CLI 執行失敗時會附上執行指令、exit code 與具體的重試建議

## URI 參考

### Agents URI

```text
[agents://]<provider>[/<token>[/<child_id>]][?<query>]
|------|  |--------|  |---------------------------|  |------|
 optional   provider         optional path parts        query
 scheme
```

- `scheme`：可省略的 `agents://` 前綴。省略時，`xurl` 會將輸入視為 `agents` URI 的簡寫。
- `provider`：目標 provider 名稱，例如 `agy`、`amp`、`claude`、`codex`、`copilot`、`cursor`、`gemini`、`kimi`、`opencode`、`pi`。
- `token`：主對話識別碼或角色名稱。
- `child_id`：主對話底下的子項目或 subagent 識別碼。
- `query`：可省略的鍵值參數，依情境解讀。

### 路徑範圍查詢 URI

```text
agents:///abs/path[?<query>]
agents://.[?<query>]
agents://./subdir[?<query>]
agents://..[?<query>]
agents://../repo[?<query>]
agents://~[?<query>]
agents://~/repo[?<query>]
```

- `agents:///abs/path`：標準的本機路徑查詢形式。
- `agents://.` / `agents://./subdir`：相對於目前工作目錄進行查詢。
- `agents://..` / `agents://../repo`：相對於目前工作目錄的上層目錄進行查詢。
- `agents://~` / `agents://~/repo`：相對於家目錄進行查詢。
- 路徑範圍查詢一律回傳對話清單。

### Agents 查詢

- `q=<keyword>`：依關鍵字篩選探索結果，適合依主題尋找對話。
- `limit=<n>`：限制探索結果筆數（預設 `10`），適合需要增減清單長度時使用。
- `providers=<name[,name...]>`：將路徑範圍查詢限制在指定的 provider。
- `<key>=<value>`：在寫入模式（`-d`）下，`xurl` 會轉發成 `--<key> <value>` 給 provider CLI。
- `<flag>`：在寫入模式（`-d`）下，`xurl` 會轉發成 `--<flag>` 給 provider CLI。

範例：

```text
agents://codex?q=spawn_agent&limit=10
agents:///Users/alice/work/xurl?q=refactor&providers=codex,claude
agents://.?q=refactor&providers=codex
agents://codex/<conversation_id>
agents://codex/reviewer
agents://codex?cd=%2FUsers%2Falice%2Frepo&add-dir=%2FUsers%2Falice%2Fshared
```
