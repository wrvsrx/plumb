/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

module.exports = grammar({
  name: 'plumb',

  externals: $ => [
    $._indent,
    $._same_line_child_indent,
    $._indent_after_blank,
    $._same_indent,
    $._paragraph_continue,
    $._inline_continue,
    $._dedent,
    $._verbatim_block_open,
    $._raw_tail_open,
    $.raw_code_line,
    $._inline_verbatim_token,
    $._inline_verbatim_member_token,
    $._inline_child_kind,
    $._incomplete_inline_end,
    $._eof,
  ],

  extras: _ => [/\r/],

  rules: {
    document: $ => repeat(choice($._block, $.blank_line)),

    _block: $ => choice($.verbatim_block, $.marked_block, $.paragraph),

    blank_line: _ => '\n',

    marked_block: $ => prec(2, seq(
      field('introducer', $.introducer),
      field('marker', $.marker),
      optional(field('content', alias($._head_inline_content, $.inline_content))),
      $._line_end,
      optional(field('body', $.block_body)),
    )),

    paragraph: $ => prec.right(seq(
      field('content', $.inline_content),
      $._line_end,
      optional(field('body', $.block_body)),
    )),

    block_body: $ => prec.right(seq(
      choice($._indent, $._indent_after_blank),
      field('child', $._block),
      repeat(choice(
        $.blank_line,
        seq($._same_indent, field('child', $._block)),
      )),
      $._dedent,
    )),

    verbatim_block: $ => prec(3, seq(
      field('introducer', $.introducer),
      optional(field('kind', $.verbatim_kind)),
      field('open', alias($._verbatim_block_open, $.verbatim_open)),
      $._line_end,
      field('body', repeat(alias($.raw_code_line, $.raw_text))),
    )),

    _head_inline_content: $ => seq(
      alias(token.immediate(/ +/), $.space),
      repeat($._inline_content_item),
    ),

    inline_content: $ => prec.right(repeat1($._inline_content_item)),

    _inline_content_item: $ => choice(
      $.introducer_escape,
      $.brace_escape,
      $.inline_verbatim,
      $.marked_group,
      $.anonymous_group,
      $.incomplete_marked_group,
      $.incomplete_anonymous_group,
      $.space,
      $.text,
    ),

    marked_group: $ => prec.right(seq(
      field('introducer', $.introducer),
      field('kind', $.inline_kind),
      token.immediate('{'),
      optional(field('content', $.inline_content)),
      '}',
    )),

    anonymous_group: $ => prec.right(seq(
      '{',
      optional(field('content', $.inline_content)),
      '}',
    )),

    incomplete_marked_group: $ => prec.dynamic(-2, prec.right(seq(
      field('introducer', $.introducer),
      field('kind', $.inline_kind),
      token.immediate('{'),
      optional(field('content', $.inline_content)),
      $._incomplete_inline_end,
    ))),

    incomplete_anonymous_group: $ => prec.dynamic(-2, prec.right(seq(
      '{',
      optional(field('content', $.inline_content)),
      $._incomplete_inline_end,
    ))),

    inline_verbatim: $ => prec.right(2, seq(
      field('introducer', $.introducer),
      optional(field('kind', $.verbatim_kind)),
      field('body', alias($._inline_verbatim_token, $.raw_text)),
    )),

    introducer_escape: _ => prec(4, '``'),
    brace_escape: _ => prec(4, choice('`{', '`}')),
    verbatim_open: _ => '"',
    introducer: _ => '`',
    marker: _ => /[^\s\x00-\x1f\x7f-\x9f`"{}]+/,
    inline_kind: _ => /[^\s\x00-\x1f\x7f-\x9f`"{}]+/,
    verbatim_kind: _ => /[^\s\x00-\x1f\x7f-\x9f`"{}]+/,
    space: _ => / +/,
    text: _ => /[^`{}\n\t\r ]+/,
    _line_end: $ => choice('\n', $._eof),
  },
});
