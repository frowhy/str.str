# 变更日志 · 仓库 / 发行

本文件记录**发行视角**的变更：每个 tag 冻结的「规范 · CLI · 技能包」三元组，以及仓库级
（工程、CI、文档）的改动。三个制品各自的详细变更见：

| 制品 | 变更日志 |
| --- | --- |
| 仓库 / 发行 | 本文件 |
| 格式规范（`spec` 轴） | [`SPEC-CHANGELOG.md`](SPEC-CHANGELOG.md) |
| 参考实现 `str-format`（`cli` 轴） | [`str-cli/CHANGELOG.md`](str-cli/CHANGELOG.md) |
| Agent 技能包 `str-skill`（`skill` 轴） | [`str-skill/CHANGELOG.md`](str-skill/CHANGELOG.md) |

版本号的真源是 [`VERSIONS.toml`](VERSIONS.toml)；本文件的版本矩阵必须与它一致。
格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，日期为 Asia/Shanghai。

## 发行版本矩阵

| tag | 日期 | 规范 | CLI | 技能包 | GUI |
| --- | --- | --- | --- | --- | --- |
| [`v0.7.2`](https://github.com/frowhy/str.str/releases/tag/v0.7.2) | 2026-09-23 | 1.13.0 | 0.7.2 | 0.4.2 | 0.3.2 |
| [`v0.7.1`](https://github.com/frowhy/str.str/releases/tag/v0.7.1) | 2026-09-22 | 1.13.0 | 0.7.1 | 0.4.1 | 0.3.0 |
| [`v0.7.0`](https://github.com/frowhy/str.str/releases/tag/v0.7.0) | 2026-09-20 | 1.13.0 | 0.7.0 | 0.4.0 | 0.2.0 |
| [`v0.3.0`](https://github.com/frowhy/str.str/releases/tag/v0.3.0) | 2026-09-14 | 1.9.0 | 0.3.0 | 0.3.0 | —（未纳入） |
| [`v0.2.0`](https://github.com/frowhy/str.str/releases/tag/v0.2.0) | 2026-09-14 | 1.8.0 | 0.2.0 | 0.2.0 | —（未纳入） |
| [`v0.1.1`](https://github.com/frowhy/str.str/releases/tag/v0.1.1) | 2026-09-14 | 1.7.0 | 0.1.0（未变更） | 0.1.1 | —（未纳入） |
| [`v0.1.0`](https://github.com/frowhy/str.str/releases/tag/v0.1.0) | 2026-09-14 | 1.7.0 | 0.1.0 | 0.1.0 | —（未纳入） |

> 发行 tag = `v` + CLI 版本（锚定规则，见 `VERSIONS.toml`）；规范与技能包**不各自打 tag**，
> 它们的版本随发行一起冻结在该矩阵里。

## [`v0.7.2`](https://github.com/frowhy/str.str/releases/tag/v0.7.2) — 2026-09-23

规范 1.13.0 · CLI 0.7.2 · 技能包 0.4.2 · GUI 0.3.2

### 修复

- **str-gui 0.3.0 → 0.3.1 —— 拖拽移动文件夹失效**：跨分支整组移动（多选拖到另一分支 /
  拖到左侧结构树分支）此前只放行 `payload` / `asset`，**已登记的内容文件夹（`role = "dir"`）
  被直接拒绝**（提示「仅支持移动 payload/asset 文件」）；同时「是否为目录」的判定发生在
  `move_file` **之后**（源路径已不存在 ⇒ 恒为 false），**未登记的文件夹**会被当文件读取而
  失败（提示「读取失败」）。现改为：闸门只拒绝分支条目（`node` / `branch`），内容文件夹与
  文件同路 —— 磁盘整棵子树搬迁、目标侧按 `dir` 角色登记（含 `count`）、源侧撤登记，移入内容
  文件夹时只维护 `count`；「是否为目录」一律前置到移动之前判定，并补「防自吞（目标目录位于
  源文件夹内部）」与「已在目标目录内则跳过」护栏。

### 变更

- **批量操作性能：约 70–100× 提速**（gui 0.3.2）：真机实测 936 项批量删除原需 ≈8.5 分钟
  （≈0.55 s/项）。三个叠加瓶颈与对策：① **`trash` 5.x 在 macOS 默认 `DeleteMethod::Finder`**
  （源码 `src/macos/mod.rs`：每次调用都 spawn 一个 `osascript` 去调 Finder）→ 改用
  `DeleteMethod::NsFileManager`（原生 `NSFileManager.trashItem`，Swift 微基准实测 **0.9 ms/项**），
  并把逐项调用合并为**每 200 项一次多路径调用**（`WriteSession::commit_trash`），
  且**按量分档**：**小批量（≤ `TRASH_FINDER_MAX` = 20 项）优先走 Finder 方式**以保留
  废纸篓的「放回原处」；**大批量或 Finder 调用失败则降级原生**（降级前按文件存在性过滤，
  避免对已入废纸篓的路径重复调用而整批报错）。真机验证：5 项走 Finder（无降级日志、
  1 秒完成）、30 项跨阈值走原生（1 秒完成），两者均零失败；② `._meta` 逐项
  `read_meta` + `save`（O(n²) 写放大）→ 改为**整批一次写会话**（`WriteSession`：读一次、内存累积、
  每 200 项与收尾各落盘一次）；③ 废纸篓**成功后才撤登记**（失败则保留登记）⇒ 磁盘与 `._meta`
  不会出现不一致。实测：300 项批量删除 **2 秒**（改造前 ≈135–210 秒）、零失败。
  另注：一度怀疑的 `Bundle::new` 实测只是 `canonicalize`，**不是**瓶颈（已排除）。

- **str-gui 状态栏文案统一与具体化**（gui 0.3.2）：底部状态栏与单条操作提示（约 100 处）统一为
  四类句式 —— 成功「已<动词> <对象>（<数量/位置>）。」、失败「<动作>失败：<原因>」、空态
  「<缺什么>。」、无变更「<未变更原因>。」；术语与量词统一（`访达` → Finder、条目「项」/
  路径「条」/ 目录「个」、条目名与路径一律以「」包裹）；去掉内部实现术语与中英混杂
  （`canonical 写回` → 「已保存分支信息」、`errors / warnings` → 「个错误 / 个警告」）；
  超长绝对路径改为对象化表述（「已在终端中打开分支「X」的目录。」「已拷贝 1 条路径：分支「X」的目录。」）；
  **所有位置变化类提示一律写明「来源 → 目标」**：跨分支移动「已从「源分支」移动 N 项 →「目标分支」」、
  拷贝 / 剪切「已从「分支」拷贝 N 项 → 系统剪贴板：a、b、c」、粘贴「已从系统剪贴板粘贴 N 项 →
  「分支」/「文件夹」」、外部导入「已从外部拖入导入 N 项 →「分支」」、分支移动「已把分支「X」从
  「父级」移动到「父级」」、同分支内移动与重排「已在「分支」内移动「a.md」→「子目录/」」；
  保存 / 删除 / 添加条目、关联、重命名与新建文件夹也点名具体对象（分支或路径）。
- **状态栏文案规范固化为代码内真源**（gui 0.3.2）：规则写进 `show_status` 的文档注释
  （四类句式 / 来源→目标 / 对象描述槽位 / 术语量词 / 禁用内部术语），新增 `name_list` helper
  统一「多条最多列 3 个名称 + …」；`str-gui/README.md` 增「状态栏文案规范」小节指向它。
  同批修掉的不一致：多条拷贝 / 剪切不列名称、用括号备注表达去向、条目级与分支级同一动作两套
  说法（拷贝路径 / Finder 显示 / 终端打开）、分支重命名不写「旧 → 新」、删除分支未标「不可撤销」。
- **状态栏输出覆盖补齐**（gui 0.3.2）：审查全部操作入口（73 个注册点）后补齐「静默」路径 ——
  「打开条目」此前完全无反馈（多选时无法确认打开了几项），「在 Finder 中显示」只有「多条或失败」
  才提示（单条成功静默）⇒ 两者改为**无条件提示**；4 处表单空值静默 `return`（新建子分支标题 /
  添加条目路径 / 添加关联目标 / 新建 bundle 名称与位置）、5 处「未选择分支 / 条目」静默 `return`、
  4 处「批量忙碌」静默 `return`、校验与批量重命名的「未打开 bundle / 未选分支」全部补上提示；
  两处「无变化」改为显式提示（`未重命名：名称未变化。` / `位置未变化。`）。
  批量操作**不在**此列：进度 + 汇总对话框始终弹出，本身就是可见反馈（本轮改为仅失败弹，见下条）。
- **批量操作：进度统一到状态栏 + 写入互斥门禁**（gui 0.3.2）：**移除模态进度浮层**
  （`ui/app.slint` 的 `batch-busy` 遮罩与 `progress-*` 属性全部删除），进度改由 `spawn_gui_batch`
  每个 tick **直写状态栏**（形如 `批量删除中 3/10 · a.md`），终态由 `on_batch_finished` 写规范摘要；
  **完成汇总仅在失败时弹出**（成功不打扰），失败框保留 —— 它承载状态栏放不下的「逐项失败原因」
  与状态栏点不到的「重试失败项」。遮罩原本兼任「批量期间禁止写盘」，因此同批**新增
  `batch_busy_guard`：`get_batch_busy` 门禁从 4 处扩展到 22 个写入口**（保存详情 / 分支与条目
  重命名 / 新建与删除分支 / 保存与添加条目 / 删除条目 / 新建文件夹 ×2 / 粘贴 ×2 / 拖拽落点 ×3 /
  外部拖入 / 关联 ×2 / 制作副本 / 打开 bundle / 打开最近 / 新建 bundle），保证后台批量线程与
  用户操作不会并发写同一份 `._meta`；批量期间用户仍可自由浏览与选中（只拦写操作）。
- **批量进度闪断修复**（gui 0.3.2）：进度是**直写**状态栏（绕过 `show_status`），而 `show_status`
  的 6s 自动清空定时器是单发且会被此前任意一次提示重启 —— 批量进行中这条旧定时器触发时会把
  正在显示的进度清空，直到下一个 tick 才恢复。定时器回调改为「`batch-busy` 期间不清空」；
  批量结束时的终态 `show_status` 仍会正常重启定时器，自动清空语义不变。
- **状态栏分支名回退修复**（gui 0.3.2）：`title_of_dir` 此前在分支**没有 `title`** 时原样返回
  `visit_title` 的「（无标题）」，于是状态栏出现 `已删除 5 项 → 分支「（无标题）」（可在废纸篓
  找回）。`（真机验收实测；`str init` 未带 `--title` 的 bundle 根分支即如此）——现回退到**目录名**。
- **CLI 0.7.1 → 0.7.2**：仅版本号随发行前进（`str --version` 现在输出 `str 0.7.2`），无命令面
  变更。详见 [`str-cli/CHANGELOG.md`](str-cli/CHANGELOG.md)。
- **技能包 0.4.1 → 0.4.2**：`str --version` 示例、`references/cli-reference.md` 的实测版本、
  `ensure-str.sh` 的 `DEFAULT_VERSION`、`bootstrap-rule.sh` 的期望输出同步为 `str 0.7.2` /
  `v0.7.2`。详见 [`str-skill/CHANGELOG.md`](str-skill/CHANGELOG.md)。
- `release.yml` 的 `workflow_dispatch` 默认 tag 更新为 `v0.7.2`。

---

## [`v0.7.1`](https://github.com/frowhy/str.str/releases/tag/v0.7.1) — 2026-09-22

规范 1.13.0 · CLI 0.7.1 · 技能包 0.4.1 · GUI 0.3.0

### 新增

- **str-gui 内容条目批量操作**（gui 0.3.0）：Finder 语义的**内容级**批量——
  - 多选：⌘(Ctrl) 点击切换、Shift 点击范围、⌘A 全选当前分支条目、点空白取消；
  - 批量删除：「移到废纸篓」作用于整个选区（≥2 时先确认），逐项走废纸篓 + 登记同步；
  - 批量移动：拖拽多选到目标分支（拖一个选中项 = 拖整组），逐项移动 + 登记转移；
  - 批量编辑：「批量重新命名…」（对标 Finder，替换文本 / 添加文本两种模式，冲突即整体拒绝）；
  - 全程进度反馈 + 完成汇总（逐项 ✓/✗，失败项一键重试）；批量仅作用于内容条目，
    分支组织结构不在批量范围内；CLI 不提供批量命令。

### 变更

- **GUI 视觉与交互打磨**（gui 0.3.0）：列表行对齐 Finder（行高 20pt、交替底色、实心 accent
  选中态、副文本中性灰与同字号、矢量折线箭头、全拉丁行光学居中）；导图激活与多选串台修复
  （点空白必定取消激活、同名分支与条目按唯一标识隔离、画布高亮刷新）；拖拽与文件操作修正
  （落点与插入指示同源、整组快照为落点依据、移入文件夹 / 分支根的登记改按目录比较、
  重命名与复制 / 粘贴的子目录路径修正、移动后展开并高亮落点）。
- **CLI 0.7.0 → 0.7.1**：仅版本号随发行前进（`str --version` 现在输出 `str 0.7.1`），无命令面
  变更。详见 [`str-cli/CHANGELOG.md`](str-cli/CHANGELOG.md)。
- **技能包 0.4.0 → 0.4.1**：`str --version` 示例与 `ensure-str.sh` 的 `DEFAULT_VERSION` 同步为
  `str 0.7.1` / `v0.7.1`。详见 [`str-skill/CHANGELOG.md`](str-skill/CHANGELOG.md)。
- `release.yml` 的 `workflow_dispatch` 默认 tag 更新为 `v0.7.1`。

---

## [`v0.7.0`](https://github.com/frowhy/str.str/releases/tag/v0.7.0) — 2026-09-20

规范 1.13.0 · CLI 0.7.0 · 技能包 0.4.0 · GUI 0.2.0

### 变更

- **GUI 修复（拖拽高亮闪烁与边缘滚动残留，第二轮）**：
  ① **闪烁根因补全** —— 节点级落点的 `can-drop` 开头会先 `clear-hover()`（清空全部悬停
  指示），上一轮「保留 hover-node」的判断在它**之后**执行，看到的永远是 -2，等于没修。
  现改为同分支拖拽时**无条件重新认领** `hover-node = 本节点`（行⇄面板空白间移动边框稳定
  亮起，不再闪烁），并清行指示（行高亮 / 插入线在面板空白处无意义）。
  ② **拖出窗口后边缘滚动残留** —— `mind-drag` 只在画布 / 节点落点的 `can-drop` 里置 true，
  而拖出窗口后不会再有任何 `can-drop`，残留的滚动定时器一直挂着，之后再平移画布就会窜出
  意外滚动。恢复画布层 `changed has-drag`（带守卫 `hover-node == -2`：已被节点接管时不动），
  并给节点层 exit 补上全清（守卫 `hover-node == 本节点`：成立即说明没有其它区域接管）。
  再加防御兜底：画布上任何**按下**都先结束悬停态 —— 即使某条收尾路径漏跑，平移画布也不会
  带出残留滚动。顺带删除已无调用的 `clear-hover-for-panel`。
- **GUI 修复（拖拽交互手感两处）**：
  ① **同分支内拖拽时分支激活边框闪烁** —— 拖拽在同分支内容面板的「行级落点 ⇄ 面板空白」
  间移动时，节点级落点会在 else 分支把 `hover-node` 清成 -2，而行级落点写入的又是本节点，
  两者交替导致激活边框（2px 强调色）闪烁。现节点级落点对同分支拖拽**保留** `hover-node`
  （仅当它指向其它节点时才清），并顺手清掉行指示（行高亮 / 插入线），悬停状态不再振荡。
  ② **边缘自动滚动改为悬停后触发** —— 拖到画布边缘带内先停留 320ms 再开始滚动（移出
  边缘带即重新计时），掠过边缘不再导致视口乱窜。
- **GUI 修复（拖拽离开分支后激活高亮不撤）**：拖拽悬停到某个分支（导图节点）上会高亮它，
  但**把拖拽移到别处时高亮不撤** —— 根因在「区域切换」而不是会话收尾：8 个 `can-drop`
  各自手写一份要清理的字段子集，彼此漂移，**画布空白那处漏了 `hover-node`**（于是拖到空白
  后分支一直保持激活高亮），另有几处漏 `mind-drag`（边缘滚动停不下来）。
  现统一为 `DndApi.clear-hover()`（悬停指示 + 边缘滚动开关，单一实现），**每个 `can-drop`
  先调它、再置本区域状态**；条目面板的整行「接收层」是文档化的例外（它必须保留上半区/
  下半区写入的状态）。`reset()` 改为委托 `clear-hover()` 并只加会话级字段
  （`payload` / `hover-src-*`）—— 两类清理从此各只有一份实现。
  另加**规则守护测试**：逐个扫描 `ui/app.slint` 的 `can-drop`，断言都先调用了
  `clear-hover()`（接收层除外）—— 该测试在本次重构中当场抓到我自己误删的那一处。
  「移出区域」的收尾也重新落位：**由覆盖整个节点的落点在 `changed has-drag`（拖拽移出
  本节点）时统一清理**（撤激活边框 + 本面板行指示；带守卫：仅在悬停状态仍归属本节点
  时清），行级落点区**不做**移出清理 —— Slint 核心先派发新区域 `can-drop`、后派发旧
  区域 exit，行⇄行移动时旧区的收尾会抹掉新区刚写好的状态（正是闪烁来源）；画布层
  同理不监听（避免误关节点区域的边缘滚动）。
  再补**移出即清理**：每个 DropArea 在 `changed has-drag`（拖拽离开本区域）时清理
  *自己写入的*状态 —— 导图节点撤激活边框（仅在 `hover-node` 仍指向本节点时，因为
  该事件可能晚于下一个节点的 can-drop）、条目面板三个落点区撤行高亮 / 插入线 /
  节点边框（仅在 `hover-panel` 仍归属本面板时）；画布层不监听（移到节点上时对方
  can-drop 先行，若在此清 `mind-drag` 会误关节点区域的边缘滚动）。「移出整个 mind
  区域」由整窗兜底 DropArea 的 `clear-hover` 覆盖，拖拽结束由 `end-drop` 覆盖。
- **GUI 修复（拖拽收尾状态残留）**：拖拽落空 / 取消后，拖拽期间置起的状态不会全部撤掉 ——
  根因是**清理清单被抄了两份且都会漂移**：`EntryApi.end-drop`（条目面板落点路径用）与
  `AppWindow.clear-dnd-hover`（树上 / 画布 / 节点落点路径用）各写了一遍字段，**两份都漏了
  `mind-drag`**（画布边缘自动滚动的 timer `running` 就是它，且 `mind-pos-*` 停在边缘带内
  ⇒ 取消拖拽后画布会自己持续平移）；行内 `drag-finished` 又手抄了第三份，同样漏。现改为
  唯一实现 `DndApi.reset()`，所有收尾路径（落点成功 / 落在无效区域 / 取消）统一调用。
  另修「激活状态」：`src-visit` 的不变量是「等于当前激活分支」，拖拽开始时被面板改写为
  拖拽源，取消后无人复位 —— 现在收尾经 `EntryApi.drag-ended()` 通知 Rust 复位
  （Rust 侧抽出 `drag_source_visit(&Editor)`，与选中同步共用同一实现）。
  画布 / 节点 DropArea 里原有的 `changed has-drag => mind-drag = false` 保留（它们覆盖
  「拖拽移出画布 / 节点」这一局部情形，但不能替代会话收尾）。
- **GUI 帮助菜单**：原「帮助」在 macOS 上整项缺失（只在非 macOS 出现、且只有一个「关于」），
  现补齐为四个入口并在**所有平台**可用 —— ①「**快捷键一览…**」新增对话框（本地内容，离线可用）：
  按菜单分组列出全部菜单快捷键 + 鼠标/触控手势（画布拖拽平移、小地图单击/拖动跳转、滚轮缩放、
  双击展开收起、拖放跨分支移动、右键菜单）与对话框按键（Esc/Enter）；修饰键字形按平台切换
  （macOS 显示 ⌘⌥⇧，其余平台 Ctrl+/Alt+/Shift+，与 `@keys(Control+…)` 的跨平台语义一致）；
  ②「打开格式规范（在线）」→ 仓库内 `STR-FORMAT-PROMPT.md`；③「打开仓库主页」；
  ④「报告问题 / 反馈」→ Issues。三个外链地址由 `CARGO_PKG_REPOSITORY` 派生（与「关于」里的
  仓库字段同源），点击后经系统默认程序打开并给出状态栏反馈。「关于」项保留在非 macOS
  （macOS 由系统应用菜单承担），帮助菜单在 macOS 不再缺项。快捷键表与 URL 派生各有单测覆盖。
- **GUI 修复（导图视口跳动）**：展开/收缩分支后整图会横跳一段（中小导图实测 67px）。
  根因是**视口补偿挂错了触发源**：内容尺寸变化时靠 `flick.changed content-width/height`
  触发「补偿居中偏移漂移」，但画布尺寸有 `max(4000px, …)` 下限 —— 内容变了而画布宽度
  不变时补偿**根本不触发**，内容在画布坐标系里整体平移就表现为跳动；反过来它在**缩放**时
  又会误触发，把陈旧的偏移差值算进去，与 `zoom-anchor` 的锚点补偿叠加成随机位移。
  改为直接监听 `mind-off`（经 off-watch 代理触发，Slint 的 `changed` 只能监听本元素属性）
  并按新值 clamp：off 与缩放无关，故只在真正的居中偏移变化时补偿，展开/收缩保持视口中心
  对准的内容点不动，缩放路径不再被污染。实测（同上探针脚本）：展开后横向位移 67px → 0，
  2441 节点收缩子树时 `-Δoff×zoom` 精确抵消。
- **GUI 性能审查（第二轮）**：定位并修掉三处与内容规模线性无关/二次相关的开销 ——
  ① **子分支索引**：`ordered_children` 每次调用都全量过滤 `visits`，而它被行构建与导图
  布局按节点逐个调用，两处因此都是 O(N²)（2441 节点实测 debug：`rebuild` 149ms、
  `mind_layout` 366ms，节点数只翻 3 倍耗时却翻 8 倍）。改为扫描时建一次 `children_index`
  后：`rebuild` 6.2ms、`mind_layout` 72ms（**24× / 5×**，缩放恢复线性）。
  ② **列表视图不再构建导图模型**：整树布局 + 节点模型只服务导图视图，列表视图（默认视图）
  下跳过，切视图时补同步 —— 每次动作省下整树布局（2441 节点 debug 约 72ms）。
  ③ **圆角裁剪不再走离屏层**：`clip: true` + `border-radius` 在渲染器里要「每帧建 FBO +
  把整棵子树渲染进纹理再合成」，直角裁剪只是 scissor。4 个面板（结构树 / 内容区 /
  信息侧栏 / 小地图）的裁剪移到内层直角容器（圆角观感由外层背景提供，内容都留有 ≥ 圆角
  半径的内边距，观感不变）。
  复测未回归：2441 节点导图视图拖拽 release 稳定 60fps。残留：每帧仍有 1 个离屏层
  （约 0.3~1ms，不在本仓库代码内，疑为内置控件内部圆角裁剪，暂不影响满帧）。
  另记：性能测量时不要固定窗口位置 —— 窗口被其它窗口遮挡会被系统降频（实测 60fps → 5fps，
  与代码无关）。
- **发行矩阵扩充**：`release.yml` 新增 `build-gui` job —— GUI 编辑器（Windows x86_64 MSVC /
  Linux x86_64 GNU）随 release 分发，产物 `str-gui-<tag>-<target>.tar.gz` / `.zip`；
  vendored Slint / winit 由 `scripts/vendor-*.sh` 在 CI 现场重建（base rev + sha256 钉死 +
  补丁）。macOS GUI 暂不随 release 分发。
- **CLI 0.6.0 → 0.7.0**：**命令面补齐** —— `str entry add|rm`（实体登记 / 移除登记，自动补
  指纹与 role 推断）、`str ignore add|rm|list`（`policies.ignore` 维护）、
  `str policies set`（`[policies]` 标量键写入）。至此「任何字段都必须有 CLI 写入路径」
  彻底闭合（规范 §9 / DoD 第 30 项）。详见 [`str-cli/CHANGELOG.md`](str-cli/CHANGELOG.md)。

- **规范 1.12.0 → 1.13.0**：**新增忽略名单** —— `policies.ignore`（gitignore 语义模式）与
  `policies.gitignore`（默认 `true` = 自动检测并应用 `.gitignore`，覆盖外层 git 仓库 →
  bundle 根 → 分支目录内，由外向内叠加、内层命中覆盖外层）。被忽略条目不参与清单比对、
  `str sync` 不补登、分支遍历剪枝；已显式登记的条目不受影响。`._meta` / `._schema/` /
  `._cache/` / `.lock` 与 §3.4 豁免清单并入**系统级忽略层**（恒为最内层、常开、
  不可被用户模式取反恢复）。纯放宽，无需迁移。详见
  [`SPEC-CHANGELOG.md`](SPEC-CHANGELOG.md)。
- **GUI 修复**：导图内容过多时小地图拖动范围受限 —— 视口中心可移动范围从
  「内容中心 ± 半视口」放宽为「± max(内容跨度×缩放, 视口) / 2」，任意规模内容都能拖到边缘。
- **GUI 交互**：四个带输入框的对话框（新建子分支、重命名条目、重命名分支、新建 bundle）
  弹出即**自动聚焦首个输入框**；两个重命名对话框额外**全选旧名**，直接输入即替换。
  全部对话框（含「关于」）接上 **Esc = 取消/关闭、Enter = 确定**：由包裹面板的 `FocusScope`
  承接按键冒泡（Slint 的投递顺序是「焦点项 → 父链 → 默认处理」，故 Esc 不会被窗口默认
  逻辑提前吞掉）；输入框自身的 `accepted` 走同一动作，事件被消费故不会二次触发。
- **GUI 修复**：「展开全部内容」此前按 `expanded` 集合判定可见范围，根节点没有内容条目时
  整项被误判为不可用（置灰 = 点了没用）。改为以 `e.rows`（当前渲染出的行）为口径 ——
  可用性判定与展开动作同一口径，根节点展开即包含其直系子分支。
- **GUI 小地图双形态**：超大导图下缩略图会退化成「堆叠色块」—— 内容被压到面板尺寸后
  单个节点不足 2px，逐项绘制只会互相覆盖，既看不出结构也认不出个体。现按**可分辨性**
  切换形态：节点块 ≥ 2px 时保持原块状渲染（等比缩放，面板随内容长宽比自适应）；否则改由
  Rust 侧把节点与连线**一次性烘焙成位图**（覆盖度 → 透明度、连线保留树的骨架、选中分支
  祖先链用强调色标出），两轴独立缩放把内容铺满固定面板 —— 极扁/极长的导图也能看出深度
  分栏与行结构。顺带把缩略图每帧开销从 O(N+E) 降到 O(1)：2440 节点 release 由 30fps → 满帧
  60fps。映射参数（盒尺寸 / 两轴比例 / 密度判据 / 用色）真源都在 `app.slint`，Rust 侧回读同一
  组数值，避免两处公式漂移（另修：`mind-w` / `mind-h` / 节点数此前在本轮写入前就被读取，
  导致首帧映射用的是上一轮布局的尺寸）。
- **GUI 性能**：**超大导图拖拽卡顿**修复（814 节点实测：release 46 → 60fps（满帧），
  debug 8 → 21fps；2440 节点 release 15 → 30fps）。三处结构性开销：
  ① 导图节点加**视口裁剪**（`visible` 由编译器降级为 Clip，裁剪区为空时渲染器整棵子树
  既不遍历也不绘制），每帧开销只与屏幕内节点数相关；② 连线由「每段一个矩形」（3 矩形/条，
  800 节点即 2400 个 item）改为**分块 Path**（画布按列带分块、缩略图整图一块；每段以中轴
  描边等价替换矩形填充，块包络盒各自参与裁剪判定）；
  ③ 小地图逐项 `opacity` 换成带透明度的颜色（去掉逐 item 的 Opacity 元素）。
  另修 `mind_dfs` 里按 visit 全量线性查找子节点的 O(N²) 布局开销。
- **GUI 菜单整改**：菜单栏新增「**分支**」菜单（新建子分支 / 保存分支信息 / **重命名分支…** /
  展开·收起子树 / 展开·收起内容 / 粘贴 / 删除分支，作用于当前选中分支，不适用时置灰）；
  「刷新（重新扫描磁盘）」移入「文件」（数据层面的重新载入），原「操作」更名「工具」——
  内含「在 Finder 中显示分支 / 显示内容」两个定位项与「校验 bundle」；
  「视图」新增「展开全部子树 / 收起全部子树 / 展开全部内容 / 收起全部内容」四个全局开关：
  **展开全部内容以当前渲染出来的节点（沿已展开链可见者）为基准**，不越界展开被折叠
  子树的内容；四项动作均给出状态栏反馈，其中「展开 / 收起全部内容」仅在**导图视图**
  下可用（内容面板只存在于导图节点里，列表视图下该项置灰）；
  结构树与导图节点的右键菜单补全为同一组操作（此前树菜单缺展开/收起、节点菜单缺粘贴，
  两者均缺重命名），三处共用同一批回调避免行为漂移。`编辑 → 重命名条目…` 改名以区分
  「改磁盘文件名」（条目）与「只改 `._meta.title`」（分支）。
- **GUI 新增**：「帮助 → 关于 STR 编辑器」对话框（此前无任何关于入口）—— 展示应用版本、
  str-format 库 / CLI 版本、规范版本与 str 主版本、仓库、许可证。macOS 侧遵循 HIG：
  原生应用菜单「关于」经 vendored Slint 追加补丁（`patches/extra/slint-macos-about-meta.patch`，
  运行时环境变量注入）显示应用名 / 版本 / 版权，帮助菜单在 macOS 不重复出现。
- **CLI 0.5.1 → 0.6.0**：实现忽略名单（`ignore` 模块 + `Scan` 携带 `IgnoreSet`，
  GUI 复用同一套语义）；`SPEC_VERSION` 同步 1.13.0。详见
  [`str-cli/CHANGELOG.md`](str-cli/CHANGELOG.md)。
- **技能包 0.3.3 → 0.3.4**：`spec-digest.md` / `cli-reference.md` 随规范同步（字段表、
  清单豁免、键序）。
- **规范 1.11.0 → 1.12.0**：保留目录（`._meta` / `._schema/` / `._cache/`）**免登记**、不参与
  `entries[]` 清单比对（消解「§1.3 约束 5 要求除 `._meta` 外全部登记」与实现放行的落差）；
  `str init` 与官方示例不再登记 `._schema`。纯放宽，既有已登记的 bundle 仍合法。详见
  [`SPEC-CHANGELOG.md`](SPEC-CHANGELOG.md)。
- **CLI 0.5.0 → 0.5.1**：`str init` 生成的 ROOT `._meta` 不再含 `._schema` 条目；`render_root_meta`
  去掉仅服务该条目的形参。详见 [`str-cli/CHANGELOG.md`](str-cli/CHANGELOG.md)。
- **技能包 0.3.2 → 0.3.3**：`spec-digest.md` / `cli-reference.md` 头部规范版本与 `str --version`
  示例同步为 1.12.0 / 0.5.1。
- `examples/客户运营.str/` 与 `scripts/build-example.sh` 同步（不再登记 `._schema`，spec = 1.12.0）。
- `VERSIONS.toml`：发行 tag 更新为 `v0.5.1`（status = `unreleased`，上一次发布 `v0.5.0`）。

---

## [`v0.5.0`](https://github.com/frowhy/str.str/releases/tag/v0.5.0) — 2026-09-14

规范 1.11.0 · CLI 0.5.0 · 技能包 0.3.2

### 变更

- **规范 1.10.0 → 1.11.0**：§9 的 `[uuid]` 缺省目标从 ROOT 细化为「当前节点」—— `[dir]` 指向
  分支目录时，省略 `<uuid>` 的命令以该分支为目标（`branch add` 挂靠、`branch rm` 自删并修复
  父级清单）；工具须以整份 bundle 为扫描视角，`.str` 硬边界不被向上穿越。详见
  [`SPEC-CHANGELOG.md`](SPEC-CHANGELOG.md)。
- **CLI 0.4.0 → 0.5.0**：`[uuid]` 缺省当前节点 + `node add` 在分支目录下的带原因拒绝。详见
  [`str-cli/CHANGELOG.md`](str-cli/CHANGELOG.md)。
- **技能包 0.3.1 → 0.3.2**：`[uuid]` 缺省说明更新为「当前节点」语义。
- `release.yml` 的 `workflow_dispatch` 默认 tag 更新为 `v0.5.0`。

---

## [Unreleased] — 目标 tag `v0.4.0`

### 变更

- **规范 1.9.0 → 1.10.0**：§9 全部命令的 `<dir>` 统一放宽为 `[dir]`（省略即当前工作目录；
  `init` 缺省以当前路径为基准目标）—— 与 v1.9.0 的 `[uuid]` 缺省 ROOT 同构，属纯放宽。
  连带 `spec set` 签名调整为 `<VERSION> [dir]`。详见 [`SPEC-CHANGELOG.md`](SPEC-CHANGELOG.md)。
- **CLI 0.3.0 → 0.4.0**：`[dir]` 缺省化 + `spec set` 参数顺序调整。详见
  [`str-cli/CHANGELOG.md`](str-cli/CHANGELOG.md)。
- **技能包 0.3.0 → 0.3.1**：命令索引补 `[dir]` 缺省说明，`--version` 示例同步到 0.4.0。
- `release.yml` 的 `workflow_dispatch` 默认 tag 更新为 `v0.4.0`。

---

## [`v0.3.0`](https://github.com/frowhy/str.str/releases/tag/v0.3.0) — 2026-09-14

规范 1.9.0 · CLI 0.3.0 · 技能包 0.3.0

### 新增

- **`VERSIONS.toml`** —— 版本唯一真源：规范 / CLI / 技能包三条轴 + 发行 tag 锚。
- **`scripts/check-versions.sh`** —— 版本一致性门禁：逐点比对「真源 ↔ 各声明点」，CI 在
  release / publish 的 test 阶段强制执行；tag 推送时还会校验「推送的 tag = 真源声明的 tag」。
- **四份 CHANGELOG.md**（本文件 + 规范 / CLI / 技能包各一份）。
- **SkillHub 自动发布**：`.github/workflows/publish-skillhub.yml` —— 推 `v*` tag 即发布技能包到
  SkillHub（版本门禁 → 本地预检 `--dry-run` → 正式发布）；`str-skill/` 自上一条 tag 无变化时自动跳过。
- 技能包 `SKILL.md` 增加 `version` 字段（此前技能包**没有版本号**），并补齐平台要求的发布
  frontmatter（`slug` / `displayName` / `summary` / `license`）。
- CLI 新增 `str spec set <dir> <VERSION>` 与规范 §9 对应条文、DoD 第 25 项。

### 变更

- **规范 1.8.0 → 1.9.0**：§9 的 `<uuid>` / `<anchor-uuid>` 统一放宽为 `[uuid]`（省略即 ROOT），
  并新增 `spec` 的 CLI 写入路径 —— 旧调用全部仍合法，属纯放宽。详见 [`SPEC-CHANGELOG.md`](SPEC-CHANGELOG.md)。
- **CLI 0.2.0 → 0.3.0**：0.2.0 已被 crates.io 占用且版本号不可复用，工作区内容必须换号发布
  （否则 `publish.yml` 会因「版本已存在」而静默跳过）。详见 [`str-cli/CHANGELOG.md`](str-cli/CHANGELOG.md)。
- **技能包 → 0.3.0**：always-on 触发 + 触发面扩大到一切文件写入，并声明依赖 CLI >= 0.3.0。
- `release.yml` 的 `workflow_dispatch` 默认 tag 由 `v0.1.0` 更正为 `v0.3.0`（此前长期滞后）。

### 修复

- 消除「同一份制品两个版本号」：`str-skill-<tag>.zip` 此前打包为 `v0.2.0`，而技能包文档自称
  「与规范同步到 v1.9.0」——现在技能包有自己的 `version`，并在文档里明确三条轴的对应关系。

---

## [`v0.2.0`](https://github.com/frowhy/str.str/releases/tag/v0.2.0) — 2026-09-14

规范 1.8.0 · CLI 0.2.0 · 技能包 0.2.0

### 新增

- 发布到 **crates.io**：`publish.yml`（可信发布 OIDC 与 Secret 二选一；目标版本已存在时跳过上传）；
  crate 内 `schema/` 派生副本 + `tests/schema_sync.rs` 守卫与仓库根真源逐字节一致。
- CLI 字段写入命令补齐：`str meta set` / `str entry set` / `str author add|rm`。
- `._cache/revisions.json` 基线，使 `E_REVISION_STALE` 可判定。
- 技能包 `ensure-str.sh` 第 6 步：`cargo install` 兜底安装（隔离到缓存目录）。

### 变更

- 规范 1.7.0 → 1.8.0：§4.9 排序细则明确化（`order` 缺省视为最大）、§9 命令面补齐、§9「写前校验」
  改述为「产出即合法且规范」。
- CLI 0.1.0 → 0.2.0；`ensure-str.sh` 内置默认版本同步。

### 移除

- `policies.unknown_entry`（与 `manifest` 语义重叠，从未被采纳）。

---

## [`v0.1.1`](https://github.com/frowhy/str.str/releases/tag/v0.1.1) — 2026-09-14

规范 1.7.0 · CLI 0.1.0（未变更）· 技能包 0.1.1

### 新增

- 技能包 `ensure-str.sh` 第 5 步：从 GitHub Releases 自动下载预编译二进制，并用 `SHA256SUMS.txt`
  **强制校验**（取不到校验和一律中止，不做降级）。

### 说明

- 本次仅技能包变更，crate 版本未 bump —— 当时 `publish.yml` 尚未建立，tag 与 Cargo 版本尚未绑定；
  该绑定自 v0.2.0 起由 `publish.yml` 强制（现在由 `scripts/check-versions.sh` 一并守卫）。

---

## [`v0.1.0`](https://github.com/frowhy/str.str/releases/tag/v0.1.0) — 2026-09-14

规范 1.7.0 · CLI 0.1.0 · 技能包 0.1.0

### 新增

- 首个发行：五平台预编译二进制 + `str-skill-<tag>.zip` + `SHA256SUMS.txt`。
- Rust 参考实现全命令面（20+ 子命令），§6 全部 **35 个错误码**各有破坏用例且断言精确到码。
- 自举：本仓库自身即 `.str` bundle，`str validate . --strict` 恒为 0 errors / 0 warnings。
- 规范 v1.7.0：`.str` 子 bundle 硬边界与 `role = "bundle"`、元数据豁免扩至 VCS、`W_ROOT_STRAY`
  语义修正、Schema 与 policy 冲突修正。
- Agent 技能包 `str-skill` 初版（MUST / NEVER 硬规则 + references + `ensure-str.sh`）。
