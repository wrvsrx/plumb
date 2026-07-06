# plumb 语法设计草案

> 本文使用 Markdown 记录语法设计，不是最终规范。当前阶段先记录从
> `docs/requirements.md` 推导出的语法方向、约束和待拍板问题。实现开始前，再把已定
> 部分收敛成精确 grammar。

## 1. 总体方向

当前方向：

- block level 使用 line marker + indentation。
- marked block 采用 headless 结构：marker line 只有 marker 和 attr，正文永远放在缩进
  child blocks 中。
- inline structure 使用 Lisp-like brace form：`{name ...}`。
- 普通 inline text 中尽量只有 `{` 和 escape 本身需要转义；其它标点默认字面量。
- attributes 使用 attr slot：`[tag #id .class key=value]`，只在 marker / form 的指定位置
  特殊。
- 语法尽可能局部、接近上下文无关；允许有限有状态 scanner 处理缩进、围栏长度、行首
  位置和 brace nesting。

一个健康的文档应接近：

```
# [#today]
  今天

普通文本里的 a*b、snake_case、[brackets]、A > B 都不用转义。
这是 {em brace form}，这是 {link ./syntax 语法草案}。

- [task=todo due=2026-07-10 .urgent]
  写语法草案
  先整理 block 模型。

  -
    子任务
    检查缩进规则。

: aside [.note]
  缩进只表示这个 aside 的内容。

> 引用
  引用里的第二段。
```

## 2. 语法局部性

语法应尽可能避免需要远距离状态或语义信息才能判断结构。允许 scanner 维护：

- 是否位于 line start / block start
- 当前 indentation stack
- code fence delimiter length
- brace form nesting stack

不允许 core grammar 依赖：

- link target 是否存在
- id 是否唯一
- class / tag / key 是否属于某个注册表
- workspace 文件系统状态
- extension 语义

少量跨节点语法检查可以存在，但必须只依赖当前语法树，并且有明确诊断收益。

## 3. Block containment by indentation

block 层包含关系统一由 indentation 表达：

```
MarkerLine
  ChildBlock
  ChildBlock
```

约束：

- 缩进宽度固定，倾向 2 spaces。
- tab 非法。
- 一层只能增加固定缩进宽度，不能任意多缩进。
- 只有显式 marker block 可以拥有 indented child blocks。
- 普通 paragraph 不能拥有 child blocks。
- 空行不产生结构，只分隔 blocks / paragraphs。

合法：

```
-
  item
  child paragraph

  -
    nested item
    nested child
```

非法：

```
普通段落
  这里不能凭空变成子块。
```

## 4. Heading and section

Heading 是普通 block：它只允许一个缩进 paragraph 作为标题文本，不允许承载 section
children。

源码保持平面：

```
#
  第一章

段落 A。

##
  小节

段落 B。

#
  第二章

段落 C。
```

core AST 也先保持平面：

```
Document [
  Heading(level=1, "第一章"),
  Paragraph("段落 A。"),
  Heading(level=2, "小节"),
  Paragraph("段落 B。"),
  Heading(level=1, "第二章"),
  Paragraph("段落 C。")
]
```

section tree 是 outline / section extension 从 heading sequence 派生的视图：

```
Section("第一章", [
  Paragraph("段落 A。"),
  Section("小节", [
    Paragraph("段落 B。")
  ])
])
Section("第二章", [
  Paragraph("段落 C。")
])
```

不要要求标题下所有内容缩进。那会让长文档自然滑向缩进地狱。

## 5. Block markers

候选 block markers：

- heading：`#` repeated 1..6 at block start。
- unordered list item：`-` at block start。
- quote：`>` at block start。
- generic container：`:` at block start。
- code block：fence，保留 raw code 内容。
- thematic break：待定。

示意：

```
# [#intro]
  引言

- [task=todo due=2026-07-10]
  写文档
  item child block。

>
  引用
  quote child block。

: aside [.warn]
  container child block。
```

统一形状：

```
MarkedBlock = Marker Attr? Newline Indent Block+ Dedent
```

parser 可以先把 marked block 统一解析为 `marker + attr + children`。随后 core 对内建
marker 做 structural validation：

- heading：children 必须恰好是一个 paragraph，该 paragraph 的 inline 内容成为标题文本；
  heading 不能拥有 section children。
- list item：children 必须是一个或多个 block。
- quote：children 必须是一个或多个 block。
- container：children 数量是否允许为零待定。
- thematic break：若采用 marker block 形态，children 必须为零；也可作为独立 line marker。

待定：

- ordered list marker。
- definition / field list 是否复用 `:`，还是用独立 marker。
- empty container 是否允许。

## 6. Inline brace forms

inline structure 统一由 brace form 进入：

```
{name body}
{name [attr] body}
```

普通文本中这些不特殊：

```
a*b
snake_case
[brackets]
`backticks`
A > B
```

候选 built-in forms：

- `{em text}`
- `{strong text}`
- `{code text}`
- `{raw text}`
- `{link target text}`
- `{span [tag #id .class key=value] text}`

示例：

```
按 {kbd Ctrl+C} 复制。
见 {link ./syntax 语法草案}。
这是 {span [mark .yellow #x data-level=2] 被标记的文本}。
```

待定：

- core 是否把 `em` / `strong` / `kbd` 等都解析成 generic `Span`，还是保留少数 typed inline。
- `{link target text}` 是否作为 core typed `Link`，还是 generic form + links extension。
- form body 是否允许嵌套 block；inline form 默认只包含 inline。
- literal `}` 在 form body 中的 escape 规则。

## 7. Attributes

attribute slot 使用 bracket form：

```
[tag #id .class key=value]
```

`[` 只在 marker / form 的 attr slot 中特殊；普通文本中的 `[` 和 `]` 默认字面量。

示例：

```
# [#today]
  今天

: aside [.note data-level=2]
  内容。

这是 {span [mark .yellow] 文本}。
```

core 只校验 attr slot 形状，并保存 opaque attr：

```
Attr = { tag, id, classes, keyvals }
```

core 不校验 tag / class / key 的语义，也不校验 id 唯一性。

## 8. Tasks as attributes

任务状态走 list item attributes，不使用 Markdown checkbox，也不另设任务 modifier 通道：

```
- [task=todo]
  买牛奶

- [task=done]
  交水费

- [task=todo due=2026-07-10 .urgent]
  预约体检
```

core 只记录 item attrs。task extension 解释 `task=todo`、`task=done`、`due` 等语义。

## 9. Code blocks

code block 保留 fence 形式，避免把代码内容纳入 plumb indentation rules。

````text
```rust
fn main() {
    println!("ok");
}
```
````

待定：

- fence delimiter 是否只允许 backticks。
- fence 长度是否可变。
- language identifier 的字符集。

## 10. 中性 AST 方向

候选节点集：

```
Document = Block[]

Block  = Heading{level, attr, inlines}
       | Paragraph{inlines}
       | List{ordered, items}
       | CodeBlock{attr, lang, text}
       | BlockQuote{attr, blocks}
       | Container{name, attr, blocks}
       | ThematicBreak{attr}

Item   = { attr, blocks }

Inline = Text{str}
       | Code{text}
       | Link{target, inlines}
       | Span{name, attr, inlines}
       | RawText{str}

Attr   = { tag, id, classes, keyvals }
```

注意：这只是方向。最终 AST 应按实现前的精确语法和 extension 需求再收敛。

## 11. 仍需拍板的问题

- 名字与扩展名。
- 固定缩进宽度是否为 2 spaces。
- ordered list marker 的最终拼写。
- `:` container 与 field list / metadata 的关系。
- `{link target text}` 的 target 和 text 如何无歧义分隔。
- brace form 的 quote / escape / raw 规则。
- built-in forms 和 generic forms 的边界。
- block attr slot 是否所有 marker 都支持。
- paragraph continuation 和空行的精确规则。
- table 是否作为 core 一等结构。
