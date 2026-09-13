# str-skill — 让 AI Agent 会用、且必须用 STR

`str-skill` 是随 STR 格式一起开源的 **Agent 技能包**：它教会 AI Agent 如何读写 `.str` bundle、如何调用 `str` CLI，并**强制**所有对 `._meta` 的操作必须经过 `str` CLI，禁止手改 TOML。

- 面向**任意**支持 Skill 的编码 Agent（CodeBuddy / Claude Code / Cursor 等），不绑定具体实现。
- SKILL.md 用英文书写（便于分发），`references/` 用中文详注（与仓库规范正文一致）。

## 目录结构

```
str-skill/
├── SKILL.md                      # 技能主体：强制规则 + 读写路径 + 命令索引（英文）
├── README.md                     # 本文件（给人看）
├── CHANGELOG.md                  # 技能包自身的变更日志（版本轴独立于 CLI / 发行 tag）
├── references/
│   ├── cli-reference.md          # str CLI 逐条命令、退出码、输出形状、规范↔实现漂移
│   ├── spec-digest.md            # STR 格式规范精要（结构/命名/字段/错误码）
│   └── workflows.md              # 可直接复制的任务配方（全部实测过）
└── scripts/
    └── ensure-str.sh             # 解析（必要时构建）str CLI，打印可执行文件路径
```

## 安装

### 1. 安装 `str` CLI

任选其一：

```sh
cargo install str-format          # 从 crates.io 安装（包名 str-format，可执行文件 str）
cargo install --path str-cli      # 或从 STR 仓库源码安装，同样装进 ~/.cargo/bin
# 或仅在仓库内使用
cd str-cli && cargo build --release   # 产物：str-cli/target/release/str
```

Agent 不必自己判断走哪条路。`scripts/ensure-str.sh` 依次尝试：`$STR_BIN` → `PATH` 上的 `str` → `$STR_REPO` → 从脚本目录 / 当前目录向上查找源码产物（发现 `str-cli/Cargo.toml` 就 `cargo build --release`）→ **从 GitHub Releases 自动下载当前平台的预编译二进制** → **用 `cargo install str-format` 从 crates.io 源码编译安装**。

第 5 步（自动下载）的细节：

- 自动识别平台并映射到 release 资产名：`Darwin/arm64`、`Darwin/x86_64`、`Linux/aarch64`、`Linux/x86_64`、`Windows/x86_64`（`.zip`）共 5 种。
- **下载后必须通过该 release 的 `SHA256SUMS.txt` 逐字节校验**；校验和取不到、缺条目或不匹配一律**中止**，不做「未校验就用」的降级。
- 结果缓存到 `${XDG_CACHE_HOME:-$HOME/.cache}/str-skill/<tag>/<target>/`，命中即复用，不重复下载。
- 可用环境变量：`STR_VERSION`（版本 tag，默认 `latest`）、`STR_RELEASE_REPO`（fork 时改 `owner/repo`）、`STR_DOWNLOAD_BASE`（网络受限时指向镜像）、`STR_CACHE_DIR`、`STR_NO_DOWNLOAD=1`（禁用下载）、`STR_NO_CARGO_INSTALL=1`（禁用源码编译安装）。

第 6 步（crates.io 源码编译安装）的细节：

- 执行 `cargo install str-format --version <tag> --locked --root <缓存>`，**安装根就是缓存目录**：`${XDG_CACHE_HOME:-$HOME/.cache}/str-skill/cargo/<tag>/bin/str`；**不写 `~/.cargo/bin`**，命中即复用、不重复编译。
- 排在下载之后：源码编译首次需数分钟，而第 5 步是预编译产物且带 SHA-256 校验。
- 版本沿用同一套解析（`STR_VERSION` → GitHub `latest` → 内置默认版本），因此编译安装与下载拿到的版本一致；该 tag 未发布到 crates.io 时会失败并继续走最后的失败分支。
- 要求本机有 `cargo`（无则跳过）；`--locked` 使用 crate 内随包发布的 `Cargo.lock`，保证与仓库验证过的依赖组合一致。

全部失败则打印安装指引并**非零退出**（绝不静默降级为手改 `._meta`）。

### 2. 把 skill 装到你的 Agent

**CodeBuddy**——项目级 skill 的约定路径是 `<workspace>/.codebuddy/skills/<skill-name>/`：

```sh
mkdir -p .codebuddy/skills
cp -R /path/to/str-skill .codebuddy/skills/str-skill
```

也可以在 CodeBuddy 设置页用「导入 Skill」导入本目录。用户级 skill 放 `~/.codebuddy/skills/`，跨项目生效。

其它 Agent 按其 skill / rules 约定放置即可；若目标 Agent 只支持一份纯 Markdown 约定文件，把 `SKILL.md` 的「Hard rules」一节原样放进该文件即可保住强制力。

## 强制力从哪来

`SKILL.md` 把「必须用 CLI、禁止手改 `._meta`」写成 **MUST / NEVER 级条款**，并把「写后必须 `str sync` + `str validate --strict` 且 0 error 才算完成」定为交付前提。这是跨 Agent 通用的最强约束：它约束的是 Agent 的行为契约，而不是某个客户端的能力。

**这条规则现在没有例外**：`str meta set` / `str entry set` / `str author add|rm` 补齐了 `type` / `title` / `summary` / `note` / `order` / `tags` / `authors[]` 的写入能力，因此 `._meta` 的全部字段（结构与描述）都由 CLI 掌握，不存在「只能手改 TOML」的字段。

需要更强（客户端级）约束时，可把 SKILL.md 的 Hard rules 复制进宿主的 always-apply 规则机制，例如 CodeBuddy 的项目规则 `.codebuddy/rules/str/RULE.mdc`（frontmatter `alwaysApply: true`）。

## 与仓库其它部分的关系

| 部分 | 角色 |
| --- | --- |
| `STR-FORMAT-PROMPT.md` | 格式规范的**唯一真源** |
| `str-cli/` | 规范的 Rust 参考实现（`str` 二进制） |
| `str-skill/` | 让 Agent 正确且强制使用前两者的技能包 |
| `examples/客户运营.str/` | 官方示例 bundle（`str validate` 通过） |

`references/` 是规范的**提炼**，不是副本。一旦两者冲突，以 `STR-FORMAT-PROMPT.md` 与 `str --help` 的实际输出为准，并请提 issue 修正本 skill。

## 已知边界

v1.8.0 的实现对齐已消解此前的落差：§4.9 排序不生效、`--fix-manifest` 不写盘、`E_REVISION_STALE` 判不动、描述性字段只能手改，并把规范自身的不一致（`policies.unknown_entry`、§9「写前校验」措辞）一并收口；v1.9.0 又把 §9 最后一处参数漂移关掉 —— 凡 `[uuid]` 位置参数**省略即 ROOT**（含 `show` / `context` / `norm` / `ref add` / `branch add` / `branch rm`）。**当前残留只剩「限制」而非「不一致」**，逐条列在 `references/cli-reference.md` §5：

- **`str validate` 不检查书写顺序**：规范把顺序门禁交给 `str fmt --check`（返回 0），写命令落盘的字节本身已规范。
- **`E_REVISION_STALE` 依赖基线**：历史判定靠 `._cache/revisions.json`（派生数据，由写命令与 `str sync` 维护）；删掉该目录即关闭这项历史检查。
- **平台相关**：`str reveal` 依赖 macOS `SetFile`（缺失只提示，不改退出码）；`str export --format toml` 是简易序列化，不是 §4.9 规范形式。

## 版本

- 技能包版本：**0.2.1**
- 适配规范：**v1.9.0**（`STR-FORMAT-PROMPT.md`）
- 依赖 CLI：**>= 0.3.0**（`str spec set` 与 `[uuid]` 位置参数缺省 ROOT 自 0.3.0 起提供）
- 发布身份：slug **`str-skill`** · 展示名 **`STR 资源树`**（SkillHub）

技能包版本、CLI 版本与发行 tag 是**三条独立演进的轴**，不要求相等；对应关系登记在仓库根
[`VERSIONS.toml`](../VERSIONS.toml)，并由 `scripts/check-versions.sh` 在 CI 中守卫。
逐版变更见 [`CHANGELOG.md`](CHANGELOG.md)：0.2.1 补齐 SkillHub 发布 frontmatter 并接入自动发布、
同步规范 v1.9.0（`[uuid]` 缺省 ROOT、新增 `str spec set`）；0.2.0 增加 `cargo install` 兜底安装；
0.1.1 增加 GitHub Releases 自动下载与 SHA-256 强制校验；0.1.0 初版。

> 规范侧的版本历史（哪些版本需要迁移、工具需支持什么）见
> [`SPEC-CHANGELOG.md`](../SPEC-CHANGELOG.md)，格式细则唯一真源仍是 `STR-FORMAT-PROMPT.md`。

## 发布到 SkillHub

技能包以 slug `str-skill`、展示名 `STR 资源树` 发布到 [SkillHub](https://skillhub.cn)。
这两个值取自 `SKILL.md` 的 frontmatter，真源登记在 [`VERSIONS.toml`](../VERSIONS.toml) 的 `[skill]`，
由 `scripts/check-versions.sh` 守卫（含 `slug` 的 kebab-case 格式与 2~128 长度校验）。

`SKILL.md` 头部必须带平台要求的 frontmatter：

| 字段 | 当前值 | 说明 |
| --- | --- | --- |
| `slug` | `str-skill` | **全网唯一**、kebab-case、2~128 字符；**首次发布后不要改** —— 改了在平台上就是另一个 skill |
| `displayName` | `STR 资源树` | 对外展示名（可为中文）。**必须用 camelCase**：平台与 CLI 只认这个键，且为必填（源码 `_validate_metadata()` 缺失即报「SKILL.md 缺少 displayName」） |
| `version` | `0.2.1` | 必须是合法 SemVer；技能包内容变化时升它 |
| `summary` | 见 SKILL.md | 一句话简介 |
| `license` | `MIT` | 开源许可证 |

### 自动发布（CI）

推 `v*` tag 即触发 [`.github/workflows/publish-skillhub.yml`](../.github/workflows/publish-skillhub.yml)：
先跑版本门禁 → 本地预检（`--dry-run`，只校验 frontmatter 与打包，**不发起 HTTP 请求、不需要 Token**）
→ 正式发布。

- **鉴权**：仓库 Secret `SKILLHUB_KEY`（SkillHub 个人 API Token，形如 `skh_...`）。
  获取需先在 skillhub.cn 登录并**完成实名认证** → 个人中心 → API keys → 创建（Token 只在创建时完整显示一次）。
- **幂等**：tag 触发时会对比「上一条 tag ↔ 本次 tag」之间 `str-skill/` 是否有变化 —— 无变化即跳过。
  技能包是独立版本轴，CLI / 规范发版不必连带重发技能包；手动触发可用 `force` 覆盖。
- **不阻塞其它发布**：本工作流与 `release.yml`（GitHub Release）、`publish.yml`（crates.io）互相独立。
- **排障**：`401` Token 失效 / 已撤销 · `403` 未完成实名认证 · `409` slug 被占用 · `429` 触发限频。

### 手动发布

```sh
# 安装 CLI（仅 CLI，不带预置技能集合）
curl -fsSL https://skillhub.cn/install/install.sh | bash -s -- --cli-only
export PATH="$HOME/.local/bin:$PATH"

skillhub login --key skh_xxx --host https://api.skillhub.cn   # 登录（Token 经参数传入）
skillhub auth whoami                                           # 确认身份
skillhub publish str-skill --dry-run                           # 本地预检（无需 Token）
skillhub publish str-skill --changelog "修复 xxx，新增 yyy"     # 正式发布
```

更新与首发流程一致：保持 `slug` 不变，改 `version` 与内容，再发布一次即可。

## 许可

与 STR 仓库一致。
