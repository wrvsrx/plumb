# plumb 语法设计草案

> 本文使用 Markdown 记录语法设计，不是最终规范。当前阶段先记录从
> `docs/requirements.md` 推导出的语法方向、约束和待拍板问题。实现开始前，再把已定
> 部分收敛成精确 grammar。

## 1. 设计基线

- strictness 针对特殊拼写和结构入口：入口一旦出现，就必须完整合法。
- 普通文本中不构成语法入口的标点不应强迫转义。
- core 只产出语法 AST 和语法 diagnostics，不解释语义。
- 表面语法可以为高频写作服务，但内部 AST 应保持小而中性。

## 2. 特殊拼写，而非全局特殊字符

建议把旧公理“特殊字符永远特殊”改为：

> 特殊拼写永远特殊。

也就是说，解析器识别具体入口 pattern，而不是把每个标点在任意位置都视为非法普通
字符。

候选分类：

- inline 入口：反引号代码、bracket span / link、自动链接、inline attr、emphasis /
  strong delimiter、escape、raw text。
- block-start 入口：heading、list、quote、code fence、thematic break、block attr、
  container、definition / field list。
- item modifier 入口：list marker 后的 `@...` 区域。

待定：每个入口的确切拼写。

## 3. Attribute 归属模型

属性值形状继续以 Pandoc / HTML 为基线：

```
{tag #id .class key=value}
```

归属规则：

- block attribute 是前缀，修饰紧随其后的 block。
- span attribute 是后缀，修饰刚闭合的 span。
- list item attribute 走 list marker 后的 item modifier 区。

示意：

```
{.lead}
这是段落。

[Enter]{kbd}

- @ {.done} 买牛奶
```

不建议：

```
- {.done} 买牛奶
```

这种写法把 item attribute 和 block attribute 都写成裸 `{...}`，只靠位置判断归属，
阅读成本偏高。

待定：

- item modifier 的最终拼写：`- @ {.done}`、`-@ {.done}`，或其它。
- block attribute 是否支持多行属性块。
- heading id 是否统一用 block attribute，而不是标题尾随 `{#id}`。

## 4. Task item 方向

任务状态走 item modifier，而不是 Markdown checkbox 拼写。

推荐方向：

```
- @todo 买牛奶
- @done 交水费
- @todo {due=2026-07-10 .urgent} 预约体检
```

task extension 解释 `@todo` / `@done` / `due=...` 等语义。core 只记录 item modifier
或 item attr。

不建议保留裸 checkbox：

```
- [ ] task
- [x] task
```

原因是 `[` 已经是 inline 入口；在 list marker 后把它特殊解释成 task，会增加 core
特例和阅读歧义。

## 5. Escape 与 raw text

逐字符 escape 保留：

```
\*
\[
\{
```

同时考虑 raw text span，用于“普通文字但不解析”的场景：

```
r[a*b, snake_case, [not a link](x), A > B]
r[[text with ] inside]]
```

raw text 和 inline code 语义不同：raw text 仍是普通文字，inline code 是代码文字。

待定：

- raw text 的最终 delimiter。
- raw block 是否进入 MVP。
- escape 的可转义字符集合。

## 6. MVP core 候选结构

MVP 需要支持的 core 结构候选：

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
- 定义列表 / 字段列表（待定）
- 转义和 raw text

## 7. 中性 AST 方向

候选节点集：

```
Document = Block[]

Block  = Heading{level, attr, inlines}
       | Paragraph{attr, inlines}
       | List{attr, ordered, items}
       | CodeBlock{attr, lang, text}
       | BlockQuote{attr, blocks}
       | FieldList{attr, entries}
       | ThematicBreak{attr}
       | Container{attr, blocks}

Item   = { attr, modifiers, blocks }

Inline = Text{str}
       | Code{text}
       | Link{target, inlines}
       | LineBreak
       | Span{attr, inlines}
       | RawText{str}

Attr   = { tag, id, classes, keyvals }
```

注意：这只是方向。最终 AST 应按实现前的精确语法和 extension 需求再收敛。

## 8. 仍需拍板的问题

- 名字与扩展名。
- list marker：无序 / 有序的最终拼写。
- item modifier：`@todo` / `@done` / attribute 的确切组合规则。
- block、span、item attribute 的最终拼写和多行形式。
- emphasis / strong delimiter 的字符和合法边界规则。
- raw text 的必要性和 delimiter。
- 定义列表 / 字段列表是否进 MVP。
- 表格是否作为 core 一等结构，还是后续 extension / container 处理。
- code fence 的 delimiter 和语言标识规则。
- block 分隔、缩进、空行的精确规则。
