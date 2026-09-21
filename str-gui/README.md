# str-gui — STR bundle 桌面编辑器

**STR（Structured Tree Resource，结构化树资源）** bundle 的桌面编辑器（Rust + Slint）。
与 `str` CLI 走**同一套读写与校验代码路径**（链接 `str-format` 库）：它是同一个
`._meta` 磁盘真相的**人类交互入口**——Agent 侧用 CLI，人类用 GUI。

## 功能

- **列表 / 思维导图双视图**：无限画布缩放与平移、小地图导航与连接线、非实际大小时显示缩放比例；
- **Finder 风格内容列表**：拖拽重排、外部文件拖入；
- **内容条目批量操作**（Finder 语义）：⌘(Ctrl)/Shift 多选、⌘A 全选，批量删除（废纸篓，≥2 项
  先确认）、批量移动（拖拽多选到目标分支）、批量重新命名（替换文本 / 添加文本），
  进度反馈 + 完成汇总 + 失败项重试；批量仅作用于内容条目，不涉及分支结构；
- **分支与内容条目管理**：增删 / 重命名 / 复制粘贴 / 制作副本；分支操作与结构树、
  导图节点的右键菜单同源；
- **菜单栏**：文件 / 编辑 / **分支** / 工具 / 视图 / 外观——分支菜单含新建子分支、保存信息、
  重命名分支、展开收起、粘贴、删除；工具含「在 Finder 中显示分支 / 显示内容」与校验；
  视图含列表·导图切换、缩放与「展开/收起全部子树·内容」；
- **内置校验**：与 `str validate` 同码同源，报错引用规范错误码；
- **最近打开**：文件菜单「打开最近」直达（持久化于用户配置目录，可清除）；
- **空状态首页**：未打开 bundle 时给出「打开 / 新建 / 拖入即开」入口与 AI 协作接入指引。

## 安装

### 预编译产物（推荐）

每个 [GitHub Release](https://github.com/frowhy/str.str/releases) 附带
Windows / Linux / macOS（universal `.app`）产物，下载即用。

### 源码构建

```sh
git clone https://github.com/frowhy/str.str
cd str.str/str-gui
bash scripts/vendor-winit.sh     # 首次构建前两个 vendor 脚本都要跑
bash scripts/vendor-slint.sh
cargo build --release            # 产物：target/release/str-gui
```

vendor 脚本对 **sha256 钉死的上游源码**施加补丁，任何人 clone 后执行都得到字节一致的
依赖源码。补丁覆盖四份上游缺失能力：

| 补丁 | 补什么 |
| --- | --- |
| winit 拖拽 | `winit-0.30.13` 未实现 `draggingUpdated:`（不返回 YES 则 AppKit 不允许落下）且不上报拖拽光标位置 |
| Slint 拖入链路 | Slint 各公开分支均无「winit 拖文件事件 → `DropEvent` 带文件路径」的转换（外部文件拖入依赖它，落点靠 winit 补丁补发的 `CursorMoved`） |
| `extra/` 两份 | 拖拽落地后的追加修正：AppKit 拖放操作类型与拖拽光标位置上报 |

## 使用

1. `str-gui` 启动后打开任意 `.str` bundle（或由 Finder 双击关联打开）；
   也可在命令行直接指定：`str-gui /path/to/项目.str`；
2. 未打开 bundle 时显示起始页，提供「打开 / 新建 / 拖入即开」入口与 AI 协作接入指引；
3. 左侧结构树 / 思维导图浏览，右侧内容列表管理 payload；
4. 编辑后用菜单「工具 → 校验」自检；与 `str` CLI 交替使用时，CLI 侧记得先
   `str validate <dir> --strict` 再继续（详见 [`../str-skill/references/str-gui.md`](../str-skill/references/str-gui.md)
   的协作守则）。

## 依赖

- 源码构建需要 Rust 工具链 + Slint；预编译产物无运行时依赖；
- CLI（`str`）非必需，但建议同装，配合 Agent 工作流。

## 许可

本目录以 **`AGPL-3.0-only`** 授权（见 [LICENSE](LICENSE)）——与仓库其它部分
（`MIT OR Apache-2.0`）不同，详见根 [README](../README.md#许可)；
vendored 的第三方依赖源码保留各自上游许可证。
