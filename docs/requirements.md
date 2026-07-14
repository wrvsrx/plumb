# plumb 目标与需求

> 本文是当前阶段的设计事实源。我们从头设计语法时，先用这里的目标、非目标和需求
> 约束每个具体符号选择；`docs/spec.md` 只记录尚未完全拍板的语法草案。

## 1. 项目定位

plumb 是一门用于个人长期文档系统的 strict markup language。它不是 Markdown 方言，
也不是面向大众生态的兼容格式。它的重点是：源码可以长期维护，结构可以被可靠解析，
错误可以尽早暴露，文档可以稳定导出和迁移。

## 2. 核心目标

- **错误尽早暴露。** 格式写错、结构没闭合、缩进不合法、语法入口残缺时，必须报错，
  不能静默变成普通文本。
- **源码可长期维护。** 文档几年后仍应能被可靠解析、批量转换、重构、查询。语法不能
  依赖“看起来大概像”的容错规则。
- **语法和语义分离。** core 只负责把文本解析成结构树和语法诊断。任务、元信息、链接
  解析、锚点唯一性、导出策略都放在 extension。
- **可导出、可迁移。** 工具链必须能稳定输出 HTML / Pandoc AST / PDF 等格式。生态
  策略不是兼容 Markdown，而是可靠导出。
- **适合日常写作。** 严格不能变成到处转义、到处样板代码。高频场景（段落、标题、
  列表、链接、任务）必须足够轻。
- **语法尽量局部。** 语法应尽可能接近上下文无关，允许少量有状态 scanner 处理缩进、
  围栏长度、行首位置等文本排版结构，但不把语义解析、注册表或工作区状态放进 core
  grammar。

## 3. 非目标

- 不追求 Markdown / Djot 兼容。可以借鉴常见习惯，但不为了兼容保留歧义。
- 不追求语法表达力最大化。宁可少一点特性，也要规则简单、诊断明确。
- 不把 core 做成语义引擎。core 不知道 `.todo`、`.meta`、`kbd`、`due` 是什么意思。
- 不为了 tree-sitter 扭曲语言设计。严格解析器是权威；tree-sitter 以后只服务编辑
  体验，可以宽松。
- 不把面向多人协作或通用发布平台的需求放在个人长期使用体验之上。

## 4. 设计原则

### 4.1 严格性

严格性针对语法入口和结构良构，而不是要求每个标点在所有位置都必须转义。普通文本中
应尽量只有极少数入口需要转义。block level 目前只把行首的统一 block introducer 视为
结构入口；inline level 的入口和转义规则尚未设计。

建议公理：

> 特殊拼写永远特殊；能构成语法入口的地方必须合法，否则报错。普通文本里不构成入口的
> 字符可以按字面处理。

例如，行首 block introducer 后缺少合法 marker token，或 block attr 没有闭合时，应当
报错，不能退回普通文本：

```
`
`#{#intro
```

### 4.2 归属明确

属性必须贴在目标的结构边界上，让人和解析器都能清楚判断归属。

- block attribute 放在紧贴 block marker 的 `{...}` attr slot 中。
- list item 的任务状态、日期等机器信息也放进 attr slot，不另设 modifier 通道。

这不追求所有结构都长得一样，而追求归属局部、可见、无需向前后远距离搜索。

### 4.3 语法局部性

语法设计应尽可能保持局部、可组合、接近上下文无关。允许 scanner 维护有限状态：

- line start / block start
- 固定缩进层级
- 当前 block indentation
- head continuation / blank-line boundary

跨节点语法检查是例外而不是常规设计手段。若必须引入，必须满足：

- 只依赖当前文档的语法树
- 不依赖 extension 语义
- 不依赖文件系统、workspace 或外部注册表
- 诊断收益明显

tree-sitter 可以做宽松镜像，不能反向约束权威语法。

### 4.4 语义中立

core 产出的 AST 应是小而中性的 Pandoc-shaped tree。syntax 层先保留 opaque marker
token；后续 core lowering 再将内建 marker 归约成类型化节点。marker 不混入普通
attributes，二者在 syntax node 中并列保存。inline tree 的具体形状尚未设计。

### 4.5 Reject-but-recover

严格解析器遇到语法错误后，应报告带字节范围的诊断，并恢复到下一个安全边界继续扫描，
以便一次显示尽可能多的错误。

### 4.6 Block containment

block 层的包含关系统一由 indentation 表达，但缩进只表达 block containment，不能表达
inline、属性参数或语义关系。为了避免 YAML / Python 式缩进地狱：

- 缩进宽度固定，一层只能增加固定数量的空格（倾向 2 spaces）。
- tab 非法。
- 只有显式 marker block 可以拥有 indented child blocks。
- 普通段落不能凭空拥有缩进子块。
- heading 的 marker line 承载标题文本，不允许拥有 indented child blocks；
  section tree 由 outline / section extension 从 heading sequence 派生。

所有 marked block 使用一个统一 introducer，并统一为“marker line + 可选 subblocks”的
形状：

```
`marker{attr}? head?
  child blocks*
```

反引号目前作为 block introducer，而不是 inline escape。只有 block start 的
`` ` + MarkerToken `` 进入 marked block grammar；`#`、`-`、`>` 等字符本身不再是
block 入口。syntax 层只保存 `#`、`-` 等 opaque marker token，不在 parser 的通用 block
外形中编码 heading、list item 等类型。

block attr slot 必须紧贴 marker，中间不能有空格。marker 后的空格开始可选 head。head
在 syntax 层只保存为抽象 `InlineContent`，本文不规定其内部语法。紧接的、增加一级缩进
的普通文本行继续 head；空行结束 head；缩进后的 marked block 直接开始 children。一旦
进入 children 就不能恢复 head。

普通 paragraph 是省略 introducer 和 marker 的默认 inline leaf。leaf 不是新的语法类别：
没有 children 的 syntax node 就是 leaf。syntax 层的统一形状为：

```
SyntaxNode {
  marker: MarkerToken?,
  attrs: Attr,
  head: InlineContent,
  children: SyntaxNode[],
}
```

heading 等内建类型的结构限制在 core lowering / structural validation 中执行。例如，
heading 的 head 成为标题内容且 children 必须为空；section tree 仍由 extension 从平面
heading sequence 派生。对于 list item、quote、container，head 如何归约到 Pandoc-shaped
block children，由后续 AST 定稿决定，不属于 inline surface syntax。

## 5. Core syntax 需求

MVP core 当前只冻结 block syntax。候选 block 结构：

- 标题
- 段落
- 列表
- 引用
- 分隔线
- block 属性
- 通用块容器
- 定义列表 / 字段列表（是否进 MVP 待定）

inline syntax、raw leaf 和 code block 暂不设计，不能从旧草案推断其入口、转义或 AST。

core 输出：

```
Document
Block[]
InlineContent
Attr
Diagnostic[]
```

core 不做：

- 自动生成 heading id
- 校验 id 唯一
- 判断链接目标是否存在
- 解释 `.todo`
- 解释 `.meta`
- 解释 block tag / class / marker 的 extension 语义
- 任务查询
- HTML 语义映射

## 6. Attribute model 需求

属性值继续以 Pandoc / HTML 的形状为基线。block attr slot 使用紧贴 marker token 的
braces：

```
`marker{tag #id .class key=value}
```

core 只校验属性语法是否良构，并把各部分保存为 opaque token。core 不校验元素名、
class、id 唯一性或 key 的语义。

block marker line 可以带 head；只有真正的 subblocks 使用缩进：

```
`#{#today} 今天

普通 paragraph。

`-{task=todo due=2026-07-10 .urgent} 买牛奶
```

- block attribute 紧贴 block marker，写作 `{...}`。
- list item 的任务状态等机器语义也写成 attribute，例如 `task=todo`。
- marker token 与普通 attributes 分开保存；不能把 marker 塞入 `Attr`。

## 7. Task model 需求

任务不是 core 语义。core 只解析 item attribute；task extension 解释状态、日期和查询
字段。

推荐方向：

```
`-{task=todo} 买牛奶

`-{task=done} 交水费

`-{task=todo due=2026-07-10 .urgent} 预约体检
```

不建议保留裸 Markdown task item：

```
- [ ] task
- [x] task
```

原因：

- task 是 extension 语义，不应成为 core 的专用 block 语义。
- item-level 信息应统一进入 attr slot。

HTML 导出建议由 task extension / export extension 共同决定，例如 task list 输出
disabled checkbox 和 task class，但源码不需要保留 checkbox 拼写。

## 8. Extension 需求

第一批 extension 候选：

- `html-export`
- `pandoc-export`
- `links`
- `anchors`
- `meta`
- `tasks`
- `outline`
- `workspace`

extension 消费 core AST，各自产生自己的 diagnostics。core 有语法错误时，下游工具不得
把文档当作完全有效输入继续导出。

## 9. 需求总结

plumb 是一门严格、语义中立、适合个人长期文档系统的标记语言。core 只负责无歧义地
解析语法并报告语法错误；所有语义由 extension 解释。语法设计优先保证源码可读、归属
明确、错误可诊断、导出可靠，同时避免为了 Markdown 兼容或全局特殊字符规则牺牲日常
书写体验。
