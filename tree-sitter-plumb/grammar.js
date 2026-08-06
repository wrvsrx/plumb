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
    $.raw_code_line,
    $._inline_verbatim_token,
    $._incomplete_inline_end,
    $._incomplete_attributes_end,
    $._eof,
  ],

  extras: _ => [/[ \t\r]/],

  conflicts: $ => [[$.document]],

  word: $ => $.attribute_name,

  rules: {
    document: $ => choice(
      seq(
        repeat($.blank_line),
        field('attached', $.attached_block_group),
        repeat(choice($._block, $.blank_line)),
      ),
      repeat(choice($._block, $.blank_line)),
    ),

    _block: $ => choice($.verbatim_block, $.marked_block, $.paragraph),

    blank_line: _ => '\n',

    marked_block: $ => prec.right(seq(
      field('introducer', $.introducer),
      field('marker', $.marker),
      optional(field('attributes', choice($.attributes, $.incomplete_attributes))),
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
          optional(seq($.head_separator, field('head', $.inline_content))),
          $._line_end,
          optional(choice(
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
      field('child', choice($._block, $.attached_block_group)),
      repeat(choice(
        $.blank_line,
        seq($._same_indent, field('child', choice($._block, $.attached_block_group))),
      )),
      $._dedent,
    ))),

    verbatim_block: $ => choice(
      seq(
        field('introducer', $.introducer),
        field('attributes', $.attributes),
        $._line_end,
        field('body', repeat(alias($.raw_code_line, $.raw_text))),
      ),
      prec(3, seq(
        field('introducer', $.introducer),
        $.block_group_open,
        $._indent,
        field('attached_content', $._block),
        repeat(choice(
          $.blank_line,
          seq($._same_indent, field('attached_content', $._block)),
        )),
        $._dedent,
        '}',
        $._line_end,
        field('body', repeat(alias($.raw_code_line, $.raw_text))),
      )),
    ),

    attached_block_group: $ => prec.right(seq(
      $.block_group_open,
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
      $._line_end,
    )),

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
      $.bracket_escape,
      $.inline_verbatim,
      $.inline_element,
      $.soft_break,
      $.inline_text,
    ))),

    attached_inline_content: $ => prec.right(repeat1(choice(
      $.introducer_escape,
      $.bracket_escape,
      $.inline_verbatim,
      $.inline_element,
      $.soft_break,
      $.attached_inline_text,
    ))),

    inline_element: $ => prec.right(2, seq(
      field('introducer', $.introducer),
      field('kind', $.inline_kind),
      '[',
      optional(field('content', $.parsed_inline_content)),
      ']',
      optional(choice(
        prec(2, field('attributes', choice($.attributes, $.incomplete_attributes))),
        field('attached', $.attached_inline_group),
      )),
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
      optional(choice(
        prec(2, field('attributes', choice($.attributes, $.incomplete_attributes))),
        field('attached', $.attached_inline_group),
      )),
    ),

    attached_inline_group: $ => prec(-1, seq(
      $._attribute_open,
      optional(field('content', $.attached_inline_content)),
      '}',
    )),

    attributes: $ => prec(2, choice(
      seq(
        $._attribute_open,
        repeat(choice(
          $._attribute_newline,
          field('id', $.attribute_id),
          field('class', $.attribute_class),
          field('pair', $.attribute_pair),
        )),
        '}',
      ),
      seq(
        $._multiline_attribute_open,
        repeat(choice(
          $._attribute_newline,
          field('id', $.attribute_id),
          field('class', $.attribute_class),
          field('pair', $.attribute_pair),
        )),
        '}',
      ),
    )),

    incomplete_attributes: $ => prec.right(-1, seq(
      $._attribute_open,
      repeat(choice(
        $._attribute_newline,
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
    attribute_name: _ => token(/[^\s\x00-\x1f\x7f-\x9f\[\]{}`"#.=]+/),
    attribute_value: _ => token(choice(
      /[^\s\x00-\x1f\x7f-\x9f\[\]{}`"#.=]+/,
      /"([^"\\\x00-\x1f\x7f-\x9f]|\\["\\])*"/,
    )),

    introducer_escape: _ => prec(3, '``'),
    bracket_escape: _ => prec(4, '`]'),
    soft_break: $ => $._inline_continue,
    introducer: _ => '`',
    marker: _ => /[^\s\x00-\x1f\x7f-\x9f\[\]{}`"]+/,
    inline_kind: _ => /[^\s\x00-\x1f\x7f-\x9f\[\]{}`"]+/,
    head_separator: _ => token(prec(2, /[ \t]+/)),
    _attribute_open: _ => token(prec(2, '{')),
    _attribute_newline: _ => /\n[ ]*/,
    block_group_open: _ => token(prec(3, /\{\n/)),
    _multiline_attribute_open: _ => token(prec(3, /\{\n/)),
    text: _ => /[^`\n]+/,
    inline_text: _ => /[^`\]\n]+/,
    attached_inline_text: _ => /[^`}\n.#=]+/,
    _line_end: $ => choice('\n', $._eof),
  },
});
