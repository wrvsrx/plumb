/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

module.exports = grammar({
  name: 'plumb',

  rules: {
    document: $ => repeat(choice($.text, $._newline)),

    text: _ => /[^\n]+/,
    _newline: _ => '\n',
  },
});
