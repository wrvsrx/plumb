/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

const PREC = { escape: 3, inline: 2, text: 1 };

module.exports = grammar({
  name: 'plumb',

  externals: $ => [
    $._indent,
    $._same_indent,
    $._dedent,
    $.code_marker,
    $.raw_code_line,
    $._inline_verbatim_token,
    $._eof,
  ],

  extras: _ => [/[ \t\r]/],
  word: $ => $.attribute_name,

  rules: {
    document: $ => repeat(choice($._block, $.blank_line)),

    _block: $ => choice($.code_block, $.marked_block, $.paragraph),
    blank_line: _ => /\n/,

    marked_block: $ => prec.right(seq(
      field('introducer', $.introducer),
      field('marker', $.marker),
      optional(field('attributes', $.attributes)),
      optional(seq($.head_separator, field('head', $.inline_content))),
      $._line_end,
      optional(choice(
        field('continued_head', $.headed_body),
        field('body', $.block_body),
      )),
    )),

    headed_body: $ => prec.right(2, seq(
      $._indent,
      field('continuation', $.head_continuation),
      repeat(seq($._same_indent, field('continuation', $.head_continuation))),
      optional(choice(
        seq(
          repeat1($.blank_line),
          optional(seq(
            $._same_indent,
            field('child', $._block),
            repeat(choice($.blank_line, seq($._same_indent, field('child', $._block)))),
          )),
        ),
        seq(
          $._same_indent,
          field('child', choice($.marked_block, $.code_block)),
          repeat(choice($.blank_line, seq($._same_indent, field('child', $._block)))),
        ),
      )),
      $._dedent,
    )),

    head_continuation: $ => prec(2, seq(field('content', $.inline_content), $._line_end)),

    block_body: $ => prec.right(seq(
      repeat($.blank_line),
      $._indent,
      $._block,
      repeat(choice(
        $.blank_line,
        seq($._same_indent, $._block),
      )),
      $._dedent,
    )),

    code_block: $ => seq(
      field('introducer', $.introducer),
      field('marker', $.code_marker),
      optional(field('attributes', $.attributes)),
      $._line_end,
      field('body', repeat1(alias($.raw_code_line, $.raw_text))),
    ),

    paragraph: $ => seq(field('content', $.inline_content), $._line_end),

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

    inline_verbatim: $ => seq(
      field('source', $._inline_verbatim_token),
      optional(field('attributes', $.attributes)),
    ),

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
    attribute_value: _ => choice(/[^\s{}"]+/, /"([^"\\]|\\.)*"/),

    introducer_escape: _ => prec(PREC.escape, '``'),
    introducer: _ => '`',
    marker: _ => /[^\s\[{`"]+/,
    inline_kind: _ => /[^\s\[{`"]+/,
    head_separator: _ => token(prec(2, /[ \t]+/)),
    text: _ => prec(PREC.text, /[^`\]\n]+|\]/),
    _line_end: $ => choice('\n', $._eof),
  },
});
