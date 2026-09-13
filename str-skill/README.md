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

在 STR 仓库根目录任选其一：

```sh
cargo install --path str-cli      # 装进 ~/.cargo/bin，全局可用
# 或仅在仓库内使用
cd str-cli && cargo build --release   # 产物：str-cli/target/release/str
```

Agent 不必自己判断走哪条路：`scripts/ensure-str.sh` 会依次尝试 `$STR_BIN` → `PATH` 上的 `str` → 向上查找 `str-cli/target/release/str` → 找到源码就用 `cargo build --release` 构建，全部失败则打印安装指引并**非零退出**（绝不静默降级为手改 `._meta`）。

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

- `references/cli-reference.md` 记录了 `str` v0.1.0 与规范 §9 的**多处参数漂移**（如 `norm` 没有 `--out`、`ref rm` 用 `--ref`），以及规范 §6.1 中 `E_REVISION_STALE` 的判定强度在实现里较弱。使用前请以该文为准。
- CLI 目前无法设置 `entries[].title` / `entries[].summary` / `tags` / `authors[]`，SKILL.md 为这类字段定义了唯一的受限例外流程。

## 许可

与 STR 仓库一致。
