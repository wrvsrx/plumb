# plumb block level 语法规范

> **状态：结构设计已定稿。** 本文中的 frozen rules 是实现 block parser 的规范基线；
> “实现前需确定的细节”仍须拍板，但不得改变总体 block model。inline syntax、通用 raw
> leaf 及最终 inline AST 不在本次定稿范围内。inline envelope 的独立设计见
> `docs/inline.md`；本文仍将 `InlineContent` 视为 opaque payload。

## 0. 定稿边界

以下规则已经冻结：

- block start 的反引号是唯一 `BlockIntroducer`；两个连续反引号转义字面的行首反引号。
- introducer 后是独立于 attributes 的 `MarkerToken`；syntax node 原样保存 marker。
- 普通 marked block 具有 `marker + attrs + head + children` 的统一结构。
- block attributes 紧贴 marker，使用 `{tag #id .class key=value}` 的形状。
- head 保存为抽象 `InlineContent`，可以跨物理行；空行结束 head。
- children 只由 indentation 表达；同级结构不得依赖 marker 的视觉宽度对齐。
- 普通 paragraph 是省略 introducer 和 marker 的默认 inline leaf，不能拥有 children。
- heading 的 head 是标题内容，heading 不能拥有 children；section 是派生视图。
- 保留 marker `""` 产生 indented code block；它没有 head 或 block children，dedent 结束
  raw payload。
- syntax 层使用 `InlinePayload | RawPayload` 区分普通 marked block 与 code block。

实现前仍需确定的项目集中列在本文末尾。除这些项目外，block surface syntax 不再继续
发散。

## 1. 总体模型

block syntax 只有一个显式入口：位于 block start 的统一 block introducer。反引号已经
确定为 introducer；它在这里不是 escape，而是声明“后面开始一个 marked block”。

```text
MarkedBlock = BlockIntroducer MarkerToken BlockAttr? Head? Newline
              HeadContinuation*
              Children?

CodeBlock   = BlockIntroducer CodeMarker BlockAttr? Newline
              IndentedRawText

BlockIntroducer = "`"
CodeMarker      = "\"\""
BlockAttr       = "{" AttrItems "}"  // 必须紧贴 MarkerToken
Children        = Indent Block+ Dedent
```

示例：

```plumb
`#{#intro} 引言

`-{task=todo} 买牛奶

`>{.note} 引用内容
```

`#`、`-`、`>` 本身不是 block 入口。只有 introducer 与 marker token 组成的特殊拼写进入
marked block grammar，因此普通文本可以直接从这些字符开始。

```plumb
# 这是普通 paragraph
- 这些符号也只是普通文本
```

introducer 后缺少合法 marker token、block attr 没有闭合、缩进层级错误等情况必须报告
语法错误，不能静默退回普通文本。

两个连续 introducer 是 block-start escape：

```plumb
``# 这是以字面 ` 开头的普通 paragraph
``- 这也不是 marked block
```

escape 必须优先于 marker token 识别。解除 escape 后，上面两行分别以字面的 `` `# `` 和
`` `- `` 开头。

## 2. Syntax node

syntax parser 对所有 marked block 使用同一个节点形状：

```text
SyntaxNode {
  marker: MarkerToken?,
  attrs: Attr,
  payload: InlinePayload | RawPayload,
}

InlinePayload = { head: InlineContent, children: SyntaxNode[] }
RawPayload    = { text: RawText }
```

- `marker` 只保存 `#`、`-`、`>` 等 opaque token；syntax parser 不根据它改变通用 block
  外形。
- `attrs` 保存紧贴 marker 的 block attributes。
- `head` 是 marker line 及其 continuation 上的 inline payload。`InlineContent` 的内部语法
  尚未设计。
- `children` 只来自缩进的 subblocks。
- 普通 paragraph 是没有显式 marker 的默认 inline leaf。
- 没有 children 的 syntax node 就是 leaf，不需要另一种 leaf delimiter。
- 保留 marker `""` 选择 `RawPayload`；其他当前 marker 选择 `InlinePayload`。

marker 控制结构，而 attributes 是 opaque metadata，二者不能混在同一个 `Attr` 中：

```text
// 推荐
SyntaxNode { marker: "#", attrs: { id: "intro" }, ... }

// 不采用
SyntaxNode { attrs: { marker: "#", id: "intro" }, ... }
```

## 3. Marker token

已经确定的内建 marker token：

```text
#, ##, ...  heading level
-           unordered list item
>           quote
:           generic container
""          code block（raw payload）
```

对应源码：

```plumb
`# 一级标题
`## 二级标题
`- list item
`> quote
`:{aside .note} generic container
`""{language=rust}
  fn main() {}
```

syntax 层只保存 marker token。后续 core lowering / structural validation 才解释内建 token，
例如将 `#` 归约为一级 heading、将 `-` 归约为 list item。identifier marker、未知 marker
和 ordered list token 属于实现前仍需确定的细节。

`""` 是保留的结构 marker，syntax parser 必须识别它以切换 raw-body parsing mode；这不
要求 parser 解释 `language` 等 attributes。

## 4. Block attributes

block attr slot 紧贴 marker token，中间不允许空格：

```text
`marker{tag #id .class key=value}
```

示例：

```plumb
`#{#today} 今天

`-{task=todo due=2026-07-10 .urgent} 预约体检

`:{aside .note data-level=2} 提示
```

attribute value 使用 Pandoc / HTML-shaped 内容模型：

```text
Attr = { tag, id, classes, keyvals }
```

core 只校验 block attr slot 是否良构并保存 opaque tokens，不校验 tag、class、key 的
语义，也不校验 id 唯一性。任务状态等 item-level metadata 进入同一个 attr slot，不另设
modifier 通道。

## 5. Head

marker 后的 inline payload 称为 head。head 与 children 是 syntax node 的两个正交部分：

```text
`marker{attrs} head
  child blocks
```

head 可以跨物理行：

```plumb
`#{#intro} 这是一个比较长的
  多行标题
```

边界规则：

1. marker line 上的 inline text 开始 head。
2. 紧接的、增加一级缩进的普通文本行继续 head。
3. 单个换行属于 head continuation；空行结束 head。
4. 缩进后的 marked block 直接开始 children，不要求前置空行。
5. 一旦开始 children，就不能恢复 head。

因此：

```plumb
`- 父项目
  head 的第二行

  child paragraph

  `- child item
```

其中前两行共同组成 head；空行后的普通文本是 paragraph child；显式 `` `- `` 是另一个
child block。

如果 marker line 没有 head，而第一个 child 是普通 paragraph，需要用空行结束空 head：

```plumb
`>

  paragraph child
```

如果第一个 child 是 marked block，其 introducer 已足以表明 head 为空，不需要额外空行：

```plumb
`>
  `- child item
```

head 内部的物理换行在 `InlineContent` 中如何表示和导出，留给 inline design 决定。

## 6. Children and indentation

block containment 只由 indentation 表达：

```text
`marker head
  ChildBlock
  ChildBlock
```

约束：

- 缩进宽度固定，倾向 2 spaces。
- tab 非法。
- 一层只能增加固定缩进宽度，不能任意多缩进。
- 只有显式 marked block 可以拥有 indented children。
- 普通 paragraph 不能凭空拥有 children。
- 空行只参与 head / paragraph 边界，不产生节点。

合法：

```plumb
`- parent
  `- child
```

非法：

```plumb
普通 paragraph
  这里不能凭空成为 child。
```

## 7. Paragraph leaf

普通 paragraph 是省略 introducer 和 marker 的默认 inline leaf：

```plumb
这是普通 paragraph 的第一行，
这是同一 paragraph 的第二行。
```

连续、同缩进、没有 block introducer 的普通文本行属于同一个 paragraph；空行、dedent、
同级 marked block 或文档结束会结束它。paragraph 必须有 `InlineContent`，并且不能拥有
children。

需要 block attributes 的 paragraph 暂时可以使用 generic marked block 表达；是否将其
定为唯一正式写法仍需拍板：

```plumb
`:{p .lead} 带 block attributes 的 paragraph。
```

## 8. Code block

code block 使用保留 marker token `""`。完整入口由一个 introducer 和两个 double quotes
组成：

````plumb
`""{language=rust}
  fn main() {
      println!("hello");
  }
````

code marker line 只允许紧贴 marker 的 block attributes，不允许 head：

```text
CodeBlock = "`\"\"" BlockAttr? Newline IndentedRawText
```

下面的形式是语法错误：

````plumb
`""{language=rust} fn main() {}
````

下一缩进层不是 block children，而是 raw code payload。每个非空 code line 必须带有恰好
一层结构缩进前缀；parser 去掉该前缀，余下字节原样保存。代码自身的 spaces 和 tabs 都
位于结构前缀之后，因此不会被 plumb indentation rules 改写。

例如源码：

````plumb
`""{language=python}
  def hello():
      print("hello")
````

保存的 raw text 是：

```python
def hello():
    print("hello")
```

其他规则：

- 空行可以完全为空，不要求带结构缩进。
- 第一个回到 parent 或 ancestor indentation 的非空行结束 code block。
- 落在既非 code body indentation、也非已有 ancestor indentation 的部分 dedent 是语法
  错误。
- 结构缩进前缀中 tab 非法；去掉结构前缀后的 tab 属于 raw payload，必须保留。
- code body 中形似 introducer、marker、attribute 或 inline syntax 的文本都不解析。
- `language` 等 attributes 保持 opaque；syntax parser 不验证语言名称。

`""` 成对出现可以避免编辑器将单个 double quote 当作未闭合 delimiter；它不是 opening
fence，不需要 closing `""`。code block 只由 dedent 结束。

## 9. Structural lowering

syntax tree 保持统一，core lowering 再将内建 marker 转成 Pandoc-shaped typed nodes。当前
确定的约束只有：

- heading 的 head 成为标题内容，且 heading 不允许 children。
- heading sequence 在 core AST 中保持平面；section tree 由 outline extension 派生。
- list item、quote 和 container 可以拥有 children。
- code marker `""` 归约为 `CodeBlock { attr, text }`。
- marker token 的解释不依赖 workspace、注册表或 extension 语义。

head 不统一包装成 `Paragraph`。共享的抽象应是 `InlineContent`：heading 直接拥有标题
content；list item、quote 或 container 的 head 在 lowering 时可归约为首个 paragraph。
具体 AST 在 inline design 完成后再冻结。

## 10. 实现前需确定的细节

这些项目会影响 block parser、diagnostics 或 core AST，必须在实现开始前决定：

### 10.1 Indentation

- 每层固定宽度已经确定；最终值是否为 2 spaces。
- 空白行是否忽略其 indentation 和 trailing whitespace。
- EOF、文件末尾缺少 newline、连续 dedent 的精确处理。
- head continuation 和直接 children 是否必须共享同一个 body indentation column。

### 10.2 Marker tokens

- `MarkerToken` 的精确字符集、token boundary 和最长匹配规则。
- heading marker 的最大重复次数。
- ordered list marker 的最终拼写。
- identifier marker 是否允许；未知 marker 是语法错误还是 generic node。
- `:` container 与 field list / metadata 的关系。

### 10.3 Node constraints and lowering

- 哪些 marker 允许空 head、children 或 block attrs。
- 连续 sibling list items 如何聚合成 `List`，以及空行是否影响聚合。
- list item、quote 和 container 的 head 是否统一 lowering 为首个 `Paragraph`。
- 空 code block 是否允许，以及 raw payload 是否保留最终 newline。
- paragraph block attributes 是否正式使用 `:{p ...}`，还是另设形式。

### 10.4 Attribute grammar

- tag 和 id 是否各最多一个，重复时如何诊断。
- class 和 key/value 是否允许重复，是否保留输入顺序。
- token 的精确字符集、value quoting 和 escape rules。
- 无属性的 `{}` 是否允许。

## 11. 定稿范围外

以下内容尚未设计，但不会改变已经冻结的 block model：

- parsed inline content、inline 换行表示和完整 escape rules；inline envelope 见
  `docs/inline.md`。
- 通用 raw/verbatim leaf。
- thematic break、table 及未来新增的 block 类型。
- 最终 Pandoc-shaped inline AST 和 extension semantics。
