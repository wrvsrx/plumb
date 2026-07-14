# plumb block level 语法设计

> 本文只记录已经收敛的 block level 设计。inline syntax、raw leaf、code block 及最终
> inline AST 尚未设计；不得从本文的 `InlineContent` 占位符推断其表面拼写。

## 1. 总体模型

block syntax 只有一个显式入口：位于 block start 的统一 block introducer。当前选定
反引号作为 introducer；它在这里不是 escape，而是声明“后面开始一个 marked block”。

```text
MarkedBlock = BlockIntroducer MarkerToken BlockAttr? Head? Newline
              HeadContinuation*
              Children?

BlockIntroducer = "`"
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

## 2. Syntax node

syntax parser 对所有 marked block 使用同一个节点形状：

```text
SyntaxNode {
  marker: MarkerToken?,
  attrs: Attr,
  head: InlineContent,
  children: SyntaxNode[],
}
```

- `marker` 只保存 `#`、`-`、`>` 等 opaque token；syntax parser 不根据它改变通用 block
  外形。
- `attrs` 保存紧贴 marker 的 block attributes。
- `head` 是 marker line 及其 continuation 上的 inline payload。`InlineContent` 的内部语法
  尚未设计。
- `children` 只来自缩进的 subblocks。
- 普通 paragraph 是没有显式 marker 的默认 inline leaf。
- 没有 children 的 syntax node 就是 leaf，不需要另一种 leaf delimiter。

marker 控制结构，而 attributes 是 opaque metadata，二者不能混在同一个 `Attr` 中：

```text
// 推荐
SyntaxNode { marker: "#", attrs: { id: "intro" }, ... }

// 不采用
SyntaxNode { attrs: { marker: "#", id: "intro" }, ... }
```

## 3. Marker token

当前高频 marker token 的方向：

```text
#, ##, ...  heading level
-           unordered list item
>           quote
:           generic container
```

对应源码：

```plumb
`# 一级标题
`## 二级标题
`- list item
`> quote
`:{aside .note} generic container
```

syntax 层只保存 marker token。后续 core lowering / structural validation 才解释内建 token，
例如将 `#` 归约为一级 heading、将 `-` 归约为 list item。是否允许 identifier marker、未知
marker 如何归约，以及 ordered list 的 marker token 尚未拍板。

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

需要 block attributes 的 paragraph 可以暂时使用 generic marked block 表达；最终拼写随
marker token 集合一起拍板：

```plumb
`:{p .lead} 带 block attributes 的 paragraph。
```

## 8. Structural lowering

syntax tree 保持统一，core lowering 再将内建 marker 转成 Pandoc-shaped typed nodes。当前
确定的约束只有：

- heading 的 head 成为标题内容，且 heading 不允许 children。
- heading sequence 在 core AST 中保持平面；section tree 由 outline extension 派生。
- list item、quote 和 container 可以拥有 children。
- marker token 的解释不依赖 workspace、注册表或 extension 语义。

head 不统一包装成 `Paragraph`。共享的抽象应是 `InlineContent`：heading 直接拥有标题
content；list item、quote 或 container 的 head 在 lowering 时可归约为首个 paragraph。
具体 AST 在 inline design 完成后再冻结。

## 9. 尚未拍板

- 固定缩进宽度是否为 2 spaces。
- `MarkerToken` 的精确字符集和最长匹配规则。
- ordered list marker 的最终拼写。
- identifier marker 和未知 marker 是否允许。
- `:` container 与 field list / metadata 的关系。
- 哪些 marker 允许空 head、children 或 block attr。
- 无属性的 `{}` 是否允许。
- paragraph block attributes 的最终写法。
- inline syntax、inline attributes、换行表示和 escape rules。
- raw leaf、code block、thematic break 和 table。
