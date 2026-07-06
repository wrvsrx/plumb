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
应尽量只有极少数入口需要转义；当前设计方向是：普通 inline text 中 `{` 是主要结构
入口，其它标点默认按字面处理。

建议公理：

> 特殊拼写永远特殊；能构成语法入口的地方必须合法，否则报错。普通文本里不构成入口的
> 字符可以按字面处理。

例如，普通文本中的这些写法不应强迫转义：

```
a*b
snake_case
A > B
3-5
[brackets]
`backticks`
```

但这些明显进入语法结构却没有写完整的形式应当报错：

```
{em missing close
{link ./target
```

### 4.2 归属明确

属性必须贴在目标的结构边界上，让人和解析器都能清楚判断归属。

- block attribute 放在 block marker line 的 attr slot 中。
- inline attribute 放在 brace form 的 attr slot 中。
- list item 的任务状态、日期等机器信息也放进 attr slot，不另设 modifier 通道。

这不追求所有结构都长得一样，而追求归属局部、可见、无需向前后远距离搜索。

### 4.3 语法局部性

语法设计应尽可能保持局部、可组合、接近上下文无关。允许 scanner 维护有限状态：

- line start / block start
- 固定缩进层级
- 当前 block indentation
- code fence delimiter length
- brace form nesting stack

跨节点语法检查是例外而不是常规设计手段。若必须引入，必须满足：

- 只依赖当前文档的语法树
- 不依赖 extension 语义
- 不依赖文件系统、workspace 或外部注册表
- 诊断收益明显

tree-sitter 可以做宽松镜像，不能反向约束权威语法。

### 4.4 语义中立

core 产出的 AST 应是小而中性的 Pandoc-shaped tree。结构性构造使用类型化节点；包装性
语义走通用 `Container` / `Span` + opaque `Attr`。surface sugar 必须归约到同一棵树，
避免多个语法拼写造成多个语义节点。

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
- heading 只允许一个缩进 paragraph 作为标题文本，不允许承载 section children；
  section tree 由 outline / section extension 从 heading sequence 派生。
- code block 保留 fence 形式，代码内容不受 plumb 缩进规则影响。

所有 marked block 倾向统一为 headless 形状：

```
marker attr?
  child blocks
```

marker line 只承载结构和 metadata，不承载正文。这样 attributes 即使很长，也不会把内容
推到行尾；正文始终从缩进子块开始。

core parser 可以先把 marked block 统一解析成 `marker + attr + children`，但内建 block
的 children 数量和类型限制仍属于 core structural validation，而不是 extension 语义。
例如 heading 必须恰好包含一个 paragraph 作为标题文本；list item 必须包含一个或多个
block；heading 不能借缩进获得 section children。

### 4.7 Lisp-like but sparse

inline 结构倾向使用 Lisp-like brace form，例如 `{em text}`、`{link target text}`，以
减少普通文本中的特殊字符和转义需求。block 层不追求全 S-expression；标题、列表、引用
等高频块仍使用 line marker + indentation，以避免满屏括号。

## 5. Core syntax 需求

MVP core 只包含语法结构和语法诊断。候选结构：

- 标题
- 段落
- 强调 / 加粗
- 行内代码
- 链接
- 自动链接
- 列表
- 引用
- 代码块
- 分隔线
- 属性
- 通用块容器
- 定义列表 / 字段列表（是否进 MVP 待定）
- 转义和 raw text

core 输出：

```
Document
Block[]
Inline[]
Attr
Diagnostic[]
```

core 不做：

- 自动生成 heading id
- 校验 id 唯一
- 判断链接目标是否存在
- 解释 `.todo`
- 解释 `.meta`
- 解释 `kbd` / `mark` / `aside` 等元素名
- 任务查询
- HTML 语义映射

## 6. Attribute model 需求

属性值形状继续以 Pandoc / HTML 的形状为基线：

```
[tag #id .class key=value]
```

core 只校验属性语法是否良构，并把各部分保存为 opaque token。core 不校验元素名、
class、id 唯一性或 key 的语义。

归属模型倾向改为 marker / form 内部 attr slot。block 级结构不再有同一行 head；内容放
在缩进子块里：

```
# [#today]
  今天

这是 {kbd Enter}。

- [task=todo due=2026-07-10 .urgent]
  买牛奶
```

- block attribute 在 block marker line 上。
- inline attribute 在 brace form 内。
- list item 的任务状态等机器语义也写成 attribute，例如 `task=todo`。

## 7. Task model 需求

任务不是 core 语义。core 只解析 item attribute；task extension 解释状态、日期和查询
字段。

推荐方向：

```
- [task=todo]
  买牛奶

- [task=done]
  交水费

- [task=todo due=2026-07-10 .urgent]
  预约体检
```

不建议保留裸 Markdown task item：

```
- [ ] task
- [x] task
```

原因：

- `[` 已经是 inline span / link 的入口，在 item 开头引入特殊例外会增加歧义。
- task 是 extension 语义，不应成为 core 的专用 block 语义。
- item-level 信息应统一进入 attr slot。

HTML 导出建议由 task extension / export extension 共同决定，例如 task list 输出
disabled checkbox 和 task class，但源码不需要保留 checkbox 拼写。

## 8. Escape / raw text 需求

普通 inline text 中主要需要转义的是 `{`（以及转义符本身）。逐字符 escape 保留：

```
\{
\\
```

再考虑增加 raw text form，用于一段里面有很多 brace 或不希望解析的文本：

```
{raw a*b, snake_case, [not a link](x), A > B}
```

`raw text` 是“普通文字但不解析”；inline code 是“代码文字”。两者语义不同，不应只用
反引号兼任。

## 9. Extension 需求

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

## 10. 需求总结

plumb 是一门严格、语义中立、适合个人长期文档系统的标记语言。core 只负责无歧义地
解析语法并报告语法错误；所有语义由 extension 解释。语法设计优先保证源码可读、归属
明确、错误可诊断、导出可靠，同时避免为了 Markdown 兼容或全局特殊字符规则牺牲日常
书写体验。
