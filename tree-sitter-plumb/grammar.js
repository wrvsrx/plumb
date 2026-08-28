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

  conflicts: _ => [],

  rules: {
    document: $ => repeat(choice($._block, $.blank_line)),

    _block: $ => choice($.verbatim_block, $.marked_block, $.paragraph),

    blank_line: _ => '\n',

    marked_block: $ => prec.right(seq(
      field('introducer', $.introducer),
      field('marker', $.marker),
      choice(
        seq(
          field('head', alias($._head_inline_content, $.inline_content)),
          $._same_line_child_indent,
          field('child', choice($.verbatim_block, $.marked_block)),
          repeat(choice(
            $.blank_line,
            seq($._same_indent, field('child', $._block)),
          )),
          $._dedent,
          optional(field('raw', $.raw_tail)),
        ),
        seq(
          optional(seq(
            field('head', alias($._head_inline_content, $.inline_content)),
          )),
          $._line_end,
          optional(choice(
            field('continued_head', $.headed_body),
            field('body', $.block_body),
          )),
          optional(field('raw', $.raw_tail)),
        ),
      ),
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
          field('child', choice($.verbatim_block, $.marked_block)),
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

    raw_tail: $ => prec.dynamic(10, prec.right(seq(
      field('open', alias($._raw_tail_open, $.raw_tail_open)),
      $._line_end,
      field('body', repeat(alias($.raw_code_line, $.raw_text))),
    ))),

    verbatim_block: $ => prec(3, seq(
      field('introducer', $.introducer),
      field('open', alias($._verbatim_block_open, $.verbatim_open)),
      $._line_end,
      field('body', repeat(alias($.raw_code_line, $.raw_text))),
    )),

    paragraph: $ => seq(
      field('content', $.inline_content),
      repeat(seq(
        $._paragraph_continue,
        field('content', $.inline_content),
      )),
      $._line_end,
    ),

    _head_inline_content: $ => seq(
      alias(token.immediate(/ +/), $.space),
      repeat($._inline_content_item),
    ),

    inline_content: $ => prec.right(repeat1($._inline_content_item)),

    _inline_content_item: $ => choice(
      $.introducer_escape,
      $.bracket_escape,
      $.pipe_escape,
      $.space_escape,
      $.inline_verbatim,
      $.inline_element,
      $.incomplete_inline_element,
      $.argument_separator,
      $.space,
      $.text,
    ),

    parsed_inline_content: $ => prec.right(repeat1($._parsed_inline_content_item)),

    _parsed_inline_content_item: $ => choice(
      $.introducer_escape,
      $.bracket_escape,
      $.pipe_escape,
      $.space_escape,
      $.inline_verbatim,
      $.inline_element,
      $.soft_break,
      $.space,
      $.inline_member_text,
    ),

    inline_element: $ => prec.dynamic(2, prec.right(2, seq(
      field('introducer', $.introducer),
      $._inline_element_body,
    ))),

    _inline_element_body: $ => seq(
      field('kind', $.inline_kind),
      $._inline_member_envelope,
    ),

    _inline_member_envelope: $ => seq(
      token.immediate(prec(5, '[')),
      optional(field('argument', $.parsed_inline_content)),
      repeat(seq(
        field('separator', $.member_separator),
        choice(
          seq(
            optional($.space),
            field('argument', $.verbatim_argument),
            optional($.space),
          ),
          seq(
            optional($.space),
            field('child', $.inline_child),
            optional($.space),
          ),
          optional(field('argument', $.parsed_inline_content)),
        ),
      )),
      ']',
    ),

    verbatim_argument: $ => field(
      'body',
      alias($._inline_verbatim_member_token, $.raw_text),
    ),

    inline_child: $ => choice(
      alias($._inline_child_element, $.inline_element),
      alias($._inline_child_verbatim, $.inline_verbatim),
    ),

    _inline_child_element: $ => seq(
      field('kind', alias($._inline_child_kind, $.inline_kind)),
      $._inline_member_envelope,
    ),

    _inline_child_verbatim: $ => seq(
      field('kind', alias($._inline_child_kind, $.verbatim_kind)),
      field('body', alias($._inline_verbatim_member_token, $.raw_text)),
    ),

    incomplete_inline_element: $ => prec.dynamic(-2, prec.right(-1, seq(
      field('introducer', $.introducer),
      field('kind', $.inline_kind),
      token.immediate(prec(5, '[')),
      optional(field('argument', $.parsed_inline_content)),
      repeat(seq(
        field('separator', $.member_separator),
        choice(
          seq(
            optional($.space),
            field('argument', $.verbatim_argument),
            optional($.space),
          ),
          seq(
            optional($.space),
            field('child', $.inline_child),
            optional($.space),
          ),
          optional(field('argument', $.parsed_inline_content)),
        ),
      )),
      $._incomplete_inline_end,
    ))),

    inline_verbatim: $ => prec.right(2, seq(
      field('introducer', $.introducer),
      optional(field('kind', $.verbatim_kind)),
      field('body', alias($._inline_verbatim_token, $.raw_text)),
    )),

    introducer_escape: _ => prec(3, '``'),
    verbatim_open: _ => token(/"+/),
    raw_tail_open: _ => token(/\|"+/),
    bracket_escape: _ => prec(4, choice('`[', '`]')),
    pipe_escape: _ => prec(4, '`|'),
    space_escape: _ => prec(4, '` '),
    soft_break: $ => $._inline_continue,
    introducer: _ => '`',
    marker: _ => /[^\s\x00-\x1f\x7f-\x9f\[\]`"|]+/,
    inline_kind: _ => /[^\s\x00-\x1f\x7f-\x9f\[\]`"|]+/,
    verbatim_kind: _ => /[^\s\x00-\x1f\x7f-\x9f\[\]`"|]+/,
    member_separator: _ => token.immediate('|'),
    argument_separator: _ => '|',
    space: _ => / +/,
    text: _ => /[^`\[\]|\n\t\r ]+/,
    inline_member_text: _ => /[^`\[\]|\n\t\r ]+/,
    _line_end: $ => choice('\n', $._eof),
  },
});
