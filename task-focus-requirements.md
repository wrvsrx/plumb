# Task 关注历史与 Next 短名单：实现需求

本文是待实现的需求交接稿，不表示当前版本已支持这些行为。实现时遵守仓库 `AGENTS.md`，先更新权威规范，再实现共享层、LSP / Web，最后更新用户文档。不能仅完成 CLI。

## 1. 目标与边界

用户同时推进多条任务，需要手工标记哪些任务已在其他工作线上推进，避免重复开工。

- 同时允许任意多条任务 focused，不设互斥或 WIP 限额。
- 关注标记及历史写入任务所属文档，不以旁路数据库作为权威来源。
- 保留反复 focus / unfocus 的区间历史。
- 关注区间表示“列入当前工作线的期间”，不表示实际工作时间。
- 不自动创建、修改或关闭 events，不计算累计工时。
- 不增加 `doing` 等 workflow state；现有派生状态保持原义。
- 不增加新的优先级评分、权重、自动过期或续期机制。

## 2. 文档表示与校验

新增标准 task property：`focused`。支持单个区间值和区间列表。

单个开放区间：

```plumb
`- 实现任务关注功能
 `+ task
 `@ task-focus
 `= focused 2026-09-20T09:00:00+08:00--
```

单个已结束区间：

```plumb
`= focused 2026-09-20T09:00:00+08:00--2026-09-20T11:00:00+08:00
```

多个区间：

```plumb
`= focused
 `- 2026-09-20T09:00:00+08:00--2026-09-20T11:00:00+08:00
 `- 2026-09-20T14:00:00+08:00--
```

后两个片段表示任务的直接属性，实际写入时应缩进到 task owner 下。

规范要求：

- 时间端点为显式带时区的 RFC 3339 时间戳；比较实际时间，不按字符串比较。
- 单值与一个元素的列表语义等价。
- 列表条目使用直接 `-` 子项，每项恰好包含一个区间，不携带额外结构或内容。
- 不允许空属性、空列表、混合行内区间与区间子项，或重复 `focused` 属性。
- 按开始时间排列；同一任务内区间不得重叠，可以首尾相接。
- 结束时间不得早于开始时间；允许零长度区间。
- 最多一个开放区间，且只能位于最后。
- 不同任务的关注区间可以重叠。
- 列表中的区间条目不产生 task 或 event records。
- 格式、重复、顺序和重叠问题产生语义诊断；不修改 core syntax，不静默修正历史。

未来时间不产生“预约 focus”的含义：开放区间立即表示 focused。UI 遇到未来起点时显示绝对时间和异常提示，不显示负的持续时间。

## 3. 派生事实

共享语义 / workspace 层提供完整的 typed 关注区间历史，以及：

- `focused`：存在有效开放区间且任务未关闭。
- `focused_since`：当前开放区间的起点；未 focused 时为空。

规则：

- ready、waiting、blocked 均可 focused。
- 状态变成 waiting / blocked 时，不自动结束关注区间。
- 标记仅作用于当前任务，不传播到父、子或依赖任务。Web 树排序的聚合不改变此规则。
- done、canceled、conflicted 均不算当前 focused。
- 手工编辑导致关闭任务仍含开放区间时，报告语义诊断，不自动重写源文档。
- 关注数据无效的任务不能被当成“未 focused”推荐；`next` 应报告存在被跳过的无效任务，支持定位修复。

CEL task context 增加派生布尔值 `focused` 和 nullable timestamp `focused_since`；时间类型与现有时间字段一致。

**存在 `focused` 属性不等于当前 focused**。已结束的历史不能排除任务再次进入候选，也不能触发 Web 排序前移。

## 4. 共享操作

实现协议无关的 `focus` / `unfocus` 操作。

| 操作 | 行为 |
| --- | --- |
| 首次 focus | 写入单个开放区间 |
| 已有全部结束的历史时 focus | 追加开放区间；单值必要时转列表 |
| 已 focused 时 focus | 幂等，不重置起点，不写文档 |
| unfocus | 为开放区间补上当前结束时间 |
| 没有开放区间时 unfocus | 幂等，不写文档 |
| Complete / Cancel | 在同一次原子编辑中结束开放区间，并执行现有关闭行为 |
| 周期任务产生下一实例 | 旧实例保留历史，新实例不继承 `focused` |

补充要求：

- focus 拒绝已关闭任务；waiting / blocked 的未关闭任务允许 focus。
- 无效关注历史上的操作明确失败，不能猜测修复。
- 时间顺序因系统时钟回拨等原因无法满足时，操作失败且不写入，不伪造时间。
- Complete / Cancel 自身被拒绝时，关注历史也不得改变。
- 一次操作只取一次当前时间，测试中可注入。
- 支持没有 explicit id 的任务，复用现有 revision-bound locator。
- 通过 `plumb-edit` 的 owned syntax / syntax-aware API 修改，保留无关声明、正文、子任务和历史时间戳。
- 复用 revision guard；LSP 修改当前 buffer，Web 遵守现有磁盘与索引冲突检查。

## 5. Next 共享查询

新增协议无关查询，返回两个独立结果区：“进行中”和“待开工”。

### 5.1 进行中

- 当前范围内所有 `focused == true` 的任务，包括 waiting / blocked。
- 按 `focused_since` 升序，最久的排在前面。
- 同时间使用 workspace-relative path、source position 稳定排序。
- 展示标题、位置、现有 workflow state / 等待原因、关注起点。
- 不受候选数量上限限制；可以有界分页，但必须提供总数与继续查看入口。

### 5.2 待开工

- 条件：`state == ready && focused == false`，且相关语义事实有效。
- 使用现有 effective priority 降序。
- 优先级按现有完整依赖关系计算，再过滤 focused 任务。
- 同优先级使用 workspace-relative path、source position 稳定排序。
- 默认最多 3 条；允许显式设置 1–10 条。
- 数量上限严格按任务计数，不因保留文档或子树而超出。
- 全局平铺排列，附带文档和父任务路径作为上下文。
- 进行中数量不扣减候选名额。
- 不额外按 due、历史关注时长、fuzzy relevance 等评分。

### 5.3 查询一致性

- 两区来自同一 workspace snapshot，使用同一查询时刻。
- 复用当前有效索引及 open-buffer overlay 规则。
- 索引未完成或存在无法评估的任务时，显式报告结果不完整，不能展示为确定的完整名单。
- 区分“候选按请求取前 N 条”和“底层索引不完整”。
- 不直接截取现有 Web 树形任务页充当全局短名单。
- 不无界 hydrate 全部完整 TaskRecord；遵守现有 typed facts、store 和 bounded query 架构。

## 6. LSP 与 Neovim

必须提供可实际使用的编辑器闭环：

- 任务 Code Action：`Focus task` / `Unfocus task`，按当前事实提供。
- task folding label 保留原状态，增加简短 focused 标识；不把 focused 替换成 workflow state。
- 提供共享 `next` 查询的 LSP projection，声明 capability 与 schema version。
- 随附 Neovim 插件增加 `next` 入口，能查看两区结果、跳转任务并执行 focus / unfocus。
- 显示“已关注多久”或“标记于多久前”，不能写成“已工作多久”。
- 两区不能在客户端重新实现另一套过滤和排序。
- 更新受影响的 typed search fields / schema 时，遵守现有兼容性约定。

## 7. Web UI

### 7.1 Next 与操作入口

- Tasks 提供清晰的 `Next` 入口。
- `Next` 展示“进行中”和“待开工”，默认候选上限 3。
- 任务行或详情提供“关注 / 取消关注”操作。
- 普通 Tasks 列表也能辨认当前 focused 的任务。
- 进行中显示本轮关注持续时间及原 workflow state。
- 详情可折叠查看完整关注区间历史；不新增专用历史编辑器，源文档仍可手工编辑。
- focus 成功后刷新两区，补齐候选。
- unfocus 后任务重新参与候选排序，不保证进入前 N 条。
- 操作 pending 时禁用对应按钮；冲突或失败明确提示并刷新，不乐观覆盖文件。
- 持续时间显示随时间更新，不因此写文档。
- 桌面和窄屏均可完成上述操作。
- 遵守已有 Web mutation 权限和 same-origin 限制。

### 7.2 普通 Tasks 树：focused 优先，子任务带动整棵祖先子树前移

普通 Web Tasks 树必须让 focused 任务优先显示，同时保持 document、parent 与 subtask subtree 连续。不能仅在当前页或浏览器已加载的行中调整顺序。

定义展示聚合值 `subtree_focused`：当前保留的 task subtree 中，只要自身或任意后代当前 focused，就为 true。Document 虚拟节点同样聚合其保留任务。

排序规则：

1. 先应用当前查询和筛选，按现有规则构建保留的展示树。
2. 对保留树自底向上计算 `subtree_focused`；被过滤掉的任务不参与聚合，折叠隐藏的任务仍参与聚合。
3. 每一层 sibling subtrees 都先按 `subtree_focused` 降序排列。
4. focused 聚合值相同的 siblings，继续按用户选择的 Priority / Due / Relevance keys 排序，最后使用现有稳定 Source tie-break。
5. document 虚拟节点也遵守 focused 优先，所以另一个文件中的 focused 子任务可以带动整个 document group 前移。

focused 优先是固定的首排序规则，不因用户调换普通 sort keys 或显式清空它们而失效；UI 应清楚显示“关注优先”。不以关注起点或历史总时长增加普通 Tasks 树的排序规则。

例如，初始树为：

```text
任务 A
  A1
任务 B
  B1
  B2
```

关注 B2 后应变为：

```text
任务 B
  B2  [focused]
  B1
任务 A
  A1
```

要求：

- B 整棵子树上移，B2 在 B 的子任务中上移；不把 B2 从父任务中抽离。
- 任意深度的 focused 后代都逐层带动祖先子树前移。
- B 本身仍未 focused，不写入 B 的 `focused` 属性。可以显示“含关注任务”的不同提示，不得伪装成本人被关注。
- 多个 sibling subtrees 都含 focused 任务时，按原有普通排序规则决定它们的相对顺序。
- 取消某个后代的关注后重新聚合；若仍有其他 focused 后代，该祖先子树继续属于 focused 优先组。
- 排序在共享查询层、分页前完成；cursor/query identity 必须体现排序规则，revision 变化遵循现有 stale cursor 处理。
- 刷新排序后保持选中任务、折叠状态以及详情上下文，不按旧行号绑定选择。
- focused 优先不绕过现有筛选；需要不受 Ready 筛选影响地查看全部进行中任务时，使用 Next 的进行中区域。

本节只改变普通 Tasks 树的排序。Next 仍使用第 5 节规定的两个平铺列表及各自排序。

## 8. 验收与交付

先写协议无关测试，再覆盖适配器。至少验证：

- 三种文档形态、时区等价比较、零长度和相接区间。
- 非法格式、重复字段、倒序、重叠、多个开放区间。
- focus / unfocus 幂等、单值转列表、历史保留。
- Complete / Cancel 原子关闭关注，失败无修改，周期实例不继承历史。
- waiting / blocked 的 focused 任务仍出现在进行中。
- 有已结束历史的 ready 任务可以再次成为候选，也不会获得 focused 排序提升。
- 父子任务标记独立，多任务并行不互相取消。
- effective priority、稳定排序、严格候选上限，以及超过候选上限的进行中展示。
- Web 树的直接 focus、多层后代 focus、跨文档前移、多个 focused 子树的次级排序。
- Web 树在取消部分 / 全部后代关注、过滤、折叠、空 sort keys 和分页场景下的排序。
- Web 树排序只聚合展示事实，不修改祖先的语义标记，刷新保持 selection 与折叠状态。
- memory / persistent store / open overlay 的查询一致性。
- revision conflict、无 explicit id 的任务操作。
- LSP 和 Web 实际操作及共享结果一致性。

完成权威规范、实现、测试和用户文档的一致性检查，运行仓库要求的 warning gate 与相关测试。若更改 README 源，按仓库流程生成对应 Markdown。

交付说明列出实现内容、验证结果及任何未完成项。不得把仅有共享层或 CLI 的实现视为本需求完成。

## 交付说明

实现完成于分支 `docs/task-focus-requirements`（在本文档提交之后追加 19 个提交）。

### 实现内容

- **权威规范先行**：`standard-semantics` §10 定义 `focused` property（单个区间或 direct `-` 子项列表、按 instant 比较、升序不重叠可相接、end 不早于 start、最多一个 open interval 且最后）、派生事实与 focus/unfocus/Complete/Cancel 语义；`diagnostics` §6 增加 focus interval 诊断归类；`semantic-analysis` §8–§9 定义 CEL `focused`/`focused_since`、shared `next` 两区查询与操作契约；`editing` §6 说明 single↔list 由 edit layer 渲染。
- **共享语义**：`TaskFocus`/`FocusInterval`/`FocusProblem` 与派生 `is_focused`/`focused_since`/`focus_valid`；六个诊断码（`task.invalid-focus`、`duplicate-focus`、`unordered-focus`、`overlapping-focus`、`multiple-open-focus`、`focus-on-closed`）都不改写源文档；child-bearing `= focused` 对 attribute view 不可见，解析直接读 block children（含空属性）。
- **plumb-edit 前置能力**：带 children 的 `=` 声明的定位/替换/删除与 scalar↔list 转换（`OwnedDeclaration`、`declaration`、`set_declaration`、`append_declaration_item`、`remove_declaration` 及 revision/green 入口）。
- **操作**：`focus_task`/`unfocus_task`（offset 与 id 两种定位）幂等、拒绝 closed task、允许 waiting/blocked 的未关闭任务；无效历史或时钟回拨失败且不写入；complete/cancel 在同一次单 revision 编辑中关闭 open 区间；周期任务旧实例保留历史、新实例不继承。
- **查询与存储**：shared `next` 两区（进行中按关注起点升序且不受候选上限限制、可分页给总数；待开工在完整关系图上传播 effective priority 后过滤、按优先级降序、默认 3 可请求 1–10、严格按 task 计数）；CEL `focused`/`focused_since`；store 派生列 + migration，`SCHEMA_VERSION` 升到 11，cursor 升到 v2；无效 focus 数据被跳过并报告。
- **Tasks 树**：展示聚合 `subtree_focused` 固定首排序（document 虚拟节点同样聚合、折叠隐藏计入、被过滤不计入、清空/重排 keys 仍生效、祖先不被写入标记）。
- **适配器**：CLI `plumb task next --limit`、`plumb task focus|unfocus`；LSP `Focus task`/`Unfocus task` code action、folding 标签保留状态符号并追加关注标识、`plumb/next` schema version 1 + capability；Neovim `:PlumbNext`（两区 picker、跳转、focus/unfocus 复用 code action、只显示关注时长）；Web `List`/`Next` 两种模式与两段结果、行内与详情关注操作、可展开区间历史、随时间刷新的时长显示、`Focused first` 提示。
- **用户文档**：`docs/guide/{semantics,workspace,toolchain,editor-integration}.plumb`、`contrib/nvim/README.plumb`（README.md 已按仓库管线重新生成）、`contrib/nvim/doc/plumb.txt`、`docs/project/completed-tasks.plumb` 记录。

### 验证结果

- `cargo test --workspace`：全绿（协议无关测试覆盖三种文档形态、时区等价、零长度与相接、六类非法输入、幂等/单值转列表/历史保留、Complete/Cancel 原子关闭与失败无修改、周期不继承、两区排序与严格上限、内存/持久/open-overlay 一致、无效 focus 跳过、取消关注后的树聚合）。
- `cargo check --workspace --all-targets`：无 error/warning。
- `node --test crates/plumb-web/assets/*.test.mjs`：38 passed。
- Neovim headless：`next_e2e.lua`、`setup_unit.lua`、`folding_unit.lua` 通过。
- Chromium CDP e2e：`mobile-shell.e2e.mjs`（含 Next 两区、聚焦/取消聚焦、详情区间历史）、`task-folding.e2e.mjs`、`task-authoring.e2e.mjs` 通过。

### 未完成项与已知问题

- **提交签名**：SSH agent 转发会话在实现中途中断，`b1a5386`、`7cd356b`、`72be3b0`、`6505065`、`1178488` 五个提交未签名（其余 15 个已签名）。socket 恢复后可统一补签：`git rebase --exec 'git commit --amend --no-edit -S' 5aad2bb`。
- **既有测试失败**：`contrib/nvim/tests/search_e2e.lua` 在本机超时失败（`complete native note search`）；在本次改动之前的基线提交上同样失败，属既有环境/Neovim 版本问题，未在本次修复。
- **e2e fixture 契约**：`mobile-shell.e2e.mjs` 的 Next 与关注断言要求被服务的工作区至少含一个当前聚焦任务与一个 ready 候选（测试头部已声明）；它同时会自己通过 UI 完成一次聚焦/取消聚焦往返。
