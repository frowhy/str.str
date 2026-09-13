# str-skill — 让 AI Agent 会用、且必须用 STR

`str-skill` 是随 STR 格式一起开源的 **Agent 技能包**：它教会 AI Agent 如何读写 `.str` bundle、如何调用 `str` CLI，并**强制**所有对 `._meta` 的操作必须经过 `str` CLI，禁止手改 TOML。

- 面向**任意**支持 Skill 的编码 Agent（CodeBuddy / Claude Code / Cursor 等），不绑定具体实现。
- SKILL.md 用英文书写（便于分发），`references/` 用中文详注（与仓库规范正文一致）。

## 目录结构

```
str-skill/
├── SKILL.md                      # 技能主体：强制规则 + 读写路径 + 命令索引（英文）
├── README.md                     # 本文件（给人看）
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

Agent 不必自己判断走哪条路。`scripts/ensure-str.sh` 依次尝试：`$STR_BIN` → `PATH` 上的 `str` → `$STR_REPO` → 从脚本目录 / 当前目录向上查找源码产物（发现 `str-cli/Cargo.toml` 就 `cargo build --release`）→ **从 GitHub Releases 自动下载当前平台的预编译二进制**。

第 5 步（自动下载）的细节：

- 自动识别平台并映射到 release 资产名：`Darwin/arm64`、`Darwin/x86_64`、`Linux/aarch64`、`Linux/x86_64`、`Windows/x86_64`（`.zip`）共 5 种。
- **下载后必须通过该 release 的 `SHA256SUMS.txt` 逐字节校验**；校验和取不到、缺条目或不匹配一律**中止**，不做「未校验就用」的降级。
- 结果缓存到 `${XDG_CACHE_HOME:-$HOME/.cache}/str-skill/<tag>/<target>/`，命中即复用，不重复下载。
- 可用环境变量：`STR_VERSION`（版本 tag，默认 `latest`）、`STR_RELEASE_REPO`（fork 时改 `owner/repo`）、`STR_DOWNLOAD_BASE`（网络受限时指向镜像）、`STR_CACHE_DIR`、`STR_NO_DOWNLOAD=1`（禁用下载，只走本地查找）。

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

v1.8.0 的实现对齐已消解此前的落差：§9 参数漂移、§4.9 排序不生效、`--fix-manifest` 不写盘、`E_REVISION_STALE` 判不动、描述性字段只能手改；同时把规范自身最后两处不一致（`policies.unknown_entry`、§9「写前校验」措辞）也一并收口。**当前残留只剩「限制」而非「不一致」**，逐条列在 `references/cli-reference.md` §5：

- **`str validate` 不检查书写顺序**：规范把顺序门禁交给 `str fmt --check`（返回 0），写命令落盘的字节本身已规范。
- **`E_REVISION_STALE` 依赖基线**：历史判定靠 `._cache/revisions.json`（派生数据，由写命令与 `str sync` 维护）；删掉该目录即关闭这项历史检查。
- **平台相关**：`str reveal` 依赖 macOS `SetFile`（缺失只提示，不改退出码）；`str export --format toml` 是简易序列化，不是 §4.9 规范形式。

## 版本

技能包与规范同步到 **v1.8.0**（`STR-FORMAT-PROMPT.md`）：排序细则明确化（`order` 缺省视为最大，混排 `._meta` 首次规范化会一次性重排）、`E_REVISION_STALE` 可判定化（§6.1.1）、§9 命令面补齐（`ls` / `ref rm` 位置参数 + 四个字段写入命令）、删除 `policies.unknown_entry`、§9「写前校验」改述为「产出即合法且规范」。

## 许可

与 STR 仓库一致。
