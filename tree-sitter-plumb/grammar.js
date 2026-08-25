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
    $.raw_code_line,
    $._inline_verbatim_token,
    $._inline_child_kind,
    $._incomplete_inline_end,
    $._eof,
  ],

  extras: _ => [/[ \t\r]/],

  conflicts: $ => [
    [$.head_continuation],
    [$._next_line_attached_children],
  ],

  rules: {
    document: $ => repeat(choice($._block, $.blank_line)),

    _block: $ => choice($.verbatim_block, $.marked_block, $.paragraph),

    blank_line: _ => '\n',

    marked_block: $ => prec.right(seq(
      field('introducer', $.introducer),
      field('marker', $.marker),
      choice(
        seq(
          $.head_separator,
          $._same_line_child_indent,
          field('child', choice($.verbatim_block, $.marked_block)),
          repeat(choice(
            $.blank_line,
            seq($._same_indent, field('child', $._block)),
          )),
          $._dedent,
        ),
        seq(
          optional(seq(
            $.head_separator,
            optional(field('head', $.inline_content)),
            optional(field('attached', choice(
              $.attached_inline_group,
              $.attached_block_group,
            ))),
          )),
          $._line_end,
          optional(choice(
            field('attached_body', $.next_line_attached_body),
            field('continued_head', $.headed_body),
            field('body', $.block_body),
          )),
        ),
      ),
    )),

    headed_body: $ => prec.dynamic(2, prec.right(seq(
      $._indent,
      field('continuation', $.head_continuation),
      optional(choice(
        seq(
          $._same_indent,
          field('attached', $.attached_block_group),
          $._line_end,
          optional($._next_line_attached_children),
        ),
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

    next_line_attached_body: $ => prec.dynamic(3, prec.right(seq(
      $._indent,
      field('attached', $.attached_block_group),
      $._line_end,
      optional($._next_line_attached_children),
      $._dedent,
    ))),

    _next_line_attached_children: $ => seq(
      repeat($.blank_line),
      $._same_indent,
      field('child', $._block),
      repeat(choice(
        $.blank_line,
        seq($._same_indent, field('child', $._block)),
      )),
    ),

    head_continuation: $ => prec(2, seq(
      field('content', $.inline_content),
      repeat(seq(
        $._paragraph_continue,
        field('content', $.inline_content),
      )),
      choice(
        $._line_end,
        seq(
          field('attached', $.attached_block_group),
          $._line_end,
          optional($._next_line_attached_children),
        ),
      ),
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

    verbatim_block: $ => prec(3, seq(
      field('introducer', $.introducer),
      optional(field('kind', $.verbatim_kind)),
      field('open', alias($._verbatim_block_open, $.verbatim_open)),
      optional(seq(
        $.head_separator,
        field('attached', choice($.attached_inline_group, $.attached_block_group)),
      )),
      $._line_end,
      field('body', repeat(alias($.raw_code_line, $.raw_text))),
    )),

    attached_block_group: $ => prec.right(seq(
      $.block_group_open,
      repeat($.blank_line),
      optional(seq(
        $._indent,
        field('content', $._block),
        repeat(choice(
          $.blank_line,
          seq($._same_indent, field('content', $._block)),
        )),
        $._dedent,
      )),
      '}',
    )),

    paragraph: $ => seq(
      field('content', $.inline_content),
      repeat(seq(
        $._paragraph_continue,
        field('content', $.inline_content),
      )),
      $._line_end,
    ),

    inline_content: $ => prec.right(repeat1(choice(
      $.introducer_escape,
      $.bracket_escape,
      $.brace_escape,
      $.pipe_escape,
      $.inline_verbatim,
      $.inline_element,
      $.incomplete_inline_element,
      $.text,
    ))),

    parsed_inline_content: $ => prec.right(repeat1(choice(
      $.introducer_escape,
      $.bracket_escape,
      $.brace_escape,
      $.pipe_escape,
      $.inline_verbatim,
      $.inline_element,
      $.soft_break,
      $.open_brace_text,
      $.inline_member_text,
    ))),

    attached_inline_content: $ => prec.right(repeat1(choice(
      $.introducer_escape,
      $.bracket_escape,
      $.brace_escape,
      $.pipe_escape,
      $.inline_verbatim,
      $.inline_element,
      $.soft_break,
      $.open_brace_text,
      $.attached_inline_text,
    ))),

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
          field('argument', $.verbatim_argument),
          field('child', $.inline_child),
          optional(field('argument', $.parsed_inline_content)),
        ),
      )),
      ']',
    ),

    verbatim_argument: $ => field(
      'body',
      alias($._inline_verbatim_token, $.raw_text),
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
      field('body', alias($._inline_verbatim_token, $.raw_text)),
    ),

    incomplete_inline_element: $ => prec.dynamic(-2, prec.right(-1, seq(
      field('introducer', $.introducer),
      field('kind', $.inline_kind),
      token.immediate(prec(5, '[')),
      optional(field('argument', $.parsed_inline_content)),
      repeat(seq(
        field('separator', $.member_separator),
        choice(
          field('argument', $.verbatim_argument),
          field('child', $.inline_child),
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

    attached_inline_group: $ => prec(3, seq(
      '{',
      optional(field('content', $.attached_inline_content)),
      '}',
    )),

    introducer_escape: _ => prec(3, '``'),
    verbatim_open: _ => token(/"+/),
    bracket_escape: _ => prec(4, choice('`[', '`]')),
    brace_escape: _ => prec(4, choice('`{', '`}')),
    pipe_escape: _ => prec(4, '`|'),
    soft_break: $ => $._inline_continue,
    introducer: _ => '`',
    marker: _ => /[^\s\x00-\x1f\x7f-\x9f\[\]{}`"|]+/,
    inline_kind: _ => /[^\s\x00-\x1f\x7f-\x9f\[\]{}`"|]+/,
    verbatim_kind: _ => /[^\s\x00-\x1f\x7f-\x9f\[\]{}`"|]+/,
    member_separator: _ => token.immediate('|'),
    head_separator: _ => token(prec(2, /[ \t]+/)),
    block_group_open: _ => token(prec(3, /\{\r?\n/)),
    open_brace_text: _ => prec(-1, '{'),
    text: _ => /[^`{\n]+/,
    inline_member_text: _ => /[^`\[\]|{\n]+/,
    attached_inline_text: _ => /[^`}\n]+/,
    _line_end: $ => choice('\n', $._eof),
  },
});
