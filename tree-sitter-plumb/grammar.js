/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

const PREC = {
  escape: 4,
  verbatim: 3,
  inline: 2,
  text: 1,
};

module.exports = grammar({
  name: 'plumb',

  extras: $ => [/[ \t\r]/],

  word: $ => $.attribute_name,

  rules: {
    document: $ => repeat(choice($._block, $.blank_line)),

    _block: $ => choice(
      $.code_block,
      $.marked_block,
      $.paragraph,
    ),

    blank_line: _ => /\n/,

    marked_block: $ => prec.right(seq(
      field('introducer', $.introducer),
      field('marker', $.marker),
      optional(field('attributes', $.attributes)),
      optional(seq($.head_separator, field('head', $.inline_content))),
      $._line_end,
      optional(field('body', $.indented_body)),
    )),

    code_block: $ => prec.right(seq(
      field('introducer', $.introducer),
      field('marker', $.code_marker),
      optional(field('attributes', $.attributes)),
      $._line_end,
      field('body', repeat1($.code_line)),
    )),

    indented_body: $ => prec.right(seq(
      repeat($.blank_line),
      choice(
        $.indented_marked_block,
        $.indented_code_block,
        $.indented_paragraph,
      ),
      repeat(choice(
        $.indented_marked_block,
        $.indented_code_block,
        $.indented_paragraph,
        $.blank_line,
      )),
    )),

    indented_marked_block: $ => seq(
      field('indent', $.indent),
      field('block', $.marked_block),
    ),

    indented_code_block: $ => seq(
      field('indent', $.indent),
      field('block', $.code_block),
    ),

    indented_paragraph: $ => seq(
      field('indent', $.indent),
      field('block', $.paragraph),
    ),

    paragraph: $ => seq(
      field('content', $.inline_content),
      $._line_end,
    ),

    code_line: $ => seq(
      field('indent', $.indent),
      optional(field('content', alias($.raw_line, $.raw_text))),
      $._line_end,
    ),

    inline_content: $ => repeat1(choice(
      $.introducer_escape,
      $.inline_verbatim,
      $.inline_element,
      $.text,
    )),

    inline_element: $ => prec.right(PREC.inline, seq(
      field('introducer', $.introducer),
      field('kind', $.inline_kind),
      '[',
      optional(field('content', $.inline_content)),
      ']',
      optional(field('attributes', $.attributes)),
    )),

    inline_verbatim: $ => prec(PREC.verbatim, choice(
      seq('`"[', field('content', alias(/([^\]\n]|\][^"\n])*/, $.raw_text)), ']"'),
      seq('`""[', field('content', alias(/([^\]\n]|\][^"\n]|\]"[^"\n])*/, $.raw_text)), ']""'),
      seq('`"""[', field('content', alias(/([^\]\n]|\][^"\n]|\]"[^"\n]|\]""[^"\n])*/, $.raw_text)), ']"""'),
    )),

    attributes: $ => seq(
      '{',
      repeat(choice(
        field('tag', $.attribute_tag),
        field('id', $.attribute_id),
        field('class', $.attribute_class),
        field('pair', $.attribute_pair),
      )),
      '}',
    ),

    attribute_tag: $ => $.attribute_name,
    attribute_id: $ => seq('#', $.attribute_name),
    attribute_class: $ => seq('.', $.attribute_name),
    attribute_pair: $ => seq(
      field('key', $.attribute_name),
      '=',
      field('value', $.attribute_value),
    ),
    attribute_name: _ => /[^\s{}#.=]+/,
    attribute_value: _ => choice(
      /[^\s{}"]+/,
      /"([^"\\]|\\.)*"/,
    ),

    introducer_escape: _ => prec(PREC.escape, '``'),
    introducer: _ => '`',
    code_marker: _ => /"+/,
    marker: _ => /[^\s\[{`"]+/,
    inline_kind: _ => /[^\s\[{`"]+/,
    head_separator: _ => token(prec(2, /[ \t]+/)),
    indent: _ => token(prec(2, / +/)),
    raw_line: _ => /[^\n]+/,
    text: _ => prec(PREC.text, /[^`\]\n]+|\]/),
    _line_end: _ => '\n',
  },
});
