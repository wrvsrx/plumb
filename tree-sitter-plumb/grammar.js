/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

module.exports = grammar({
  name: 'plumb',

  externals: $ => [
    $._indent,
    $._indent_after_blank,
    $._same_indent,
    $._paragraph_continue,
    $._dedent,
    $.code_marker,
    $.raw_code_line,
    $._inline_verbatim_token,
    $._incomplete_inline_end,
    $._incomplete_attributes_end,
    $._eof,
  ],

  extras: _ => [/[ \t\r]/],

  word: $ => $.attribute_name,

  rules: {
    document: $ => repeat(choice($._block, $.blank_line)),

    _block: $ => choice($.code_block, $.marked_block, $.paragraph),

    blank_line: _ => '\n',

    marked_block: $ => prec.right(seq(
      field('introducer', $.introducer),
      field('marker', $.marker),
      optional(field('attributes', choice($.attributes, $.incomplete_attributes))),
      optional(seq($.head_separator, field('head', $.inline_content))),
      $._line_end,
      optional(choice(
        field('continued_head', $.headed_body),
        field('body', $.block_body),
      )),
    )),

    headed_body: $ => prec.dynamic(2, prec.right(seq(
      $._indent,
      field('continuation', $.head_continuation),
      optional(choice(
        seq(
          repeat1($.blank_line),
          optional(seq(
            $._same_indent,
            field('child', $._block),
            repeat(choice(
              $.blank_line,
              seq($._same_indent, field('child', $._block)),
            )),
          )),
        ),
        seq(
          $._same_indent,
          field('child', choice($.code_block, $.marked_block)),
          repeat(choice(
            $.blank_line,
            seq($._same_indent, field('child', $._block)),
          )),
        ),
      )),
      $._dedent,
    ))),

    head_continuation: $ => prec(2, seq(
      field('content', $.inline_content),
      repeat(seq(
        $._paragraph_continue,
        field('content', $.inline_content),
      )),
      $._line_end,
    )),

    block_body: $ => prec.dynamic(1, prec.right(seq(
      choice($._indent, $._indent_after_blank),
      field('child', $._block),
      repeat(choice(
        $.blank_line,
        seq($._same_indent, field('child', $._block)),
      )),
      $._dedent,
    ))),

    code_block: $ => seq(
      field('introducer', $.introducer),
      field('marker', $.code_marker),
      optional(field('attributes', $.attributes)),
      $._line_end,
      field('body', repeat(alias($.raw_code_line, $.raw_text))),
    ),

    paragraph: $ => seq(
      field('content', $.inline_content),
      repeat(seq(
        $._paragraph_continue,
        field('content', $.inline_content),
      )),
      $._line_end,
    ),

    inline_content: $ => repeat1(choice(
      $.introducer_escape,
      $.inline_verbatim,
      $.inline_element,
      $.incomplete_inline_element,
      $.text,
    )),

    parsed_inline_content: $ => prec.right(repeat1(choice(
      $.introducer_escape,
      $.inline_verbatim,
      $.inline_element,
      $.inline_text,
    ))),

    inline_element: $ => prec.right(2, seq(
      field('introducer', $.introducer),
      field('kind', $.inline_kind),
      '[',
      optional(field('content', $.parsed_inline_content)),
      ']',
      optional(field('attributes', choice($.attributes, $.incomplete_attributes))),
    )),

    incomplete_inline_element: $ => prec.right(-1, seq(
      field('introducer', $.introducer),
      field('kind', $.inline_kind),
      '[',
      optional(field('content', $.parsed_inline_content)),
      $._incomplete_inline_end,
    )),

    inline_verbatim: $ => seq(
      field('source', $._inline_verbatim_token),
      optional(field('attributes', choice($.attributes, $.incomplete_attributes))),
    ),

    attributes: $ => seq(
      $._attribute_open,
      repeat(choice(
        field('id', $.attribute_id),
        field('class', $.attribute_class),
        field('pair', $.attribute_pair),
      )),
      '}',
    ),

    incomplete_attributes: $ => prec.right(-1, seq(
      $._attribute_open,
      repeat(choice(
        field('id', $.attribute_id),
        field('class', $.attribute_class),
        field('pair', $.attribute_pair),
      )),
      $._incomplete_attributes_end,
    )),

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

    introducer_escape: _ => prec(3, '``'),
    introducer: _ => '`',
    marker: _ => /[^\s\x00-\x1f\x7f-\x9f\[\]{}`"]+/,
    inline_kind: _ => /[^\s\x00-\x1f\x7f-\x9f\[\]{}`"]+/,
    head_separator: _ => token(prec(2, /[ \t]+/)),
    _attribute_open: _ => token(prec(2, '{')),
    text: _ => /[^`\n]+/,
    inline_text: _ => /[^`\]\n]+/,
    _line_end: $ => choice('\n', $._eof),
  },
});
