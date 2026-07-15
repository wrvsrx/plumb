/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

module.exports = grammar({
  name: 'plumb',

  extras: _ => [/[ \t\r]/],

  word: $ => $.attribute_name,

  rules: {
    document: $ => repeat(choice($._block, $.blank_line)),

    _block: $ => choice($.marked_block, $.paragraph),

    blank_line: _ => '\n',

    marked_block: $ => seq(
      field('introducer', $.introducer),
      field('marker', $.marker),
      optional(field('attributes', $.attributes)),
      optional(seq($.head_separator, field('head', $.inline_content))),
      '\n',
    ),

    paragraph: $ => prec.right(seq(
      field('content', $.inline_content),
      repeat(seq('\n', field('content', $.inline_content))),
      '\n',
    )),

    inline_content: $ => repeat1($.text),

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

    introducer: _ => '`',
    marker: _ => /[^\s\[{`"]+/,
    head_separator: _ => token(prec(2, /[ \t]+/)),
    text: _ => /[^`\n]+/,
  },
});
