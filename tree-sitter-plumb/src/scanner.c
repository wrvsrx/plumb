#include "tree_sitter/parser.h"

#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

enum TokenType {
  INDENT,
  INDENT_AFTER_BLANK,
  SAME_INDENT,
  PARAGRAPH_CONTINUE,
  INLINE_CONTINUE,
  DEDENT,
  RAW_CODE_LINE,
  INLINE_VERBATIM_TOKEN,
  INCOMPLETE_INLINE_END,
  INCOMPLETE_ATTRIBUTES_END,
  END_OF_FILE,
};

#define MAX_INDENT_DEPTH 64

typedef struct {
  uint16_t indents[MAX_INDENT_DEPTH];
  uint8_t depth;
  uint8_t pending_dedents;
} Scanner;

static void skip(TSLexer *lexer) { lexer->advance(lexer, true); }
static void take(TSLexer *lexer) { lexer->advance(lexer, false); }

void *tree_sitter_plumb_external_scanner_create(void) {
  return calloc(1, sizeof(Scanner));
}

void tree_sitter_plumb_external_scanner_destroy(void *payload) {
  free(payload);
}

unsigned tree_sitter_plumb_external_scanner_serialize(void *payload,
                                                       char *buffer) {
  Scanner *scanner = payload;
  unsigned size = 0;
  buffer[size++] = (char)scanner->depth;
  buffer[size++] = (char)scanner->pending_dedents;
  for (uint8_t i = 0; i <= scanner->depth; i++) {
    buffer[size++] = (char)(scanner->indents[i] & 0xff);
    buffer[size++] = (char)(scanner->indents[i] >> 8);
  }
  return size;
}

void tree_sitter_plumb_external_scanner_deserialize(void *payload,
                                                     const char *buffer,
                                                     unsigned length) {
  Scanner *scanner = payload;
  scanner->depth = 0;
  scanner->pending_dedents = 0;
  scanner->indents[0] = 0;
  if (length < 2) return;

  scanner->depth = (uint8_t)buffer[0];
  if (scanner->depth >= MAX_INDENT_DEPTH) scanner->depth = MAX_INDENT_DEPTH - 1;
  scanner->pending_dedents = (uint8_t)buffer[1];
  unsigned available = (length - 2) / 2;
  if (available <= scanner->depth) scanner->depth = available ? available - 1 : 0;
  for (uint8_t i = 0; i <= scanner->depth; i++) {
    unsigned offset = 2u + (unsigned)i * 2u;
    if (offset + 1u >= length) break;
    scanner->indents[i] = (uint8_t)buffer[offset] |
                          ((uint16_t)(uint8_t)buffer[offset + 1u] << 8);
  }
}

static bool scan_raw_code_line(Scanner *scanner, TSLexer *lexer,
                               const bool *valid_symbols) {
  lexer->mark_end(lexer);
  uint16_t verbatim_indent = scanner->indents[scanner->depth] + 2;
  uint16_t spaces = 0;
  while (lexer->lookahead == ' ' && spaces < verbatim_indent) {
    take(lexer);
    spaces++;
  }

  if (lexer->lookahead == '\n') {
    if (spaces > 0 && spaces < verbatim_indent) return false;
    take(lexer);
    lexer->mark_end(lexer);
    lexer->result_symbol = RAW_CODE_LINE;
    return true;
  }
  if (spaces < verbatim_indent) {
    uint16_t current = scanner->indents[scanner->depth];
    if (spaces == current && current > 0 && valid_symbols[SAME_INDENT]) {
      lexer->mark_end(lexer);
      lexer->result_symbol = SAME_INDENT;
      return true;
    }
    if (spaces < current && valid_symbols[DEDENT]) {
      uint8_t target = scanner->depth;
      while (target > 0 && scanner->indents[target] > spaces) target--;
      scanner->pending_dedents = scanner->depth - target - 1;
      scanner->depth--;
      lexer->result_symbol = DEDENT;
      return true;
    }
    return false;
  }
  if (lexer->lookahead == 0) return false;

  while (lexer->lookahead != '\n' && lexer->lookahead != 0) take(lexer);
  if (lexer->lookahead == '\n') take(lexer);
  lexer->mark_end(lexer);
  lexer->result_symbol = RAW_CODE_LINE;
  return true;
}

static bool scan_inline_verbatim(TSLexer *lexer) {
  if (lexer->lookahead != '`') return false;
  take(lexer);

  uint16_t quotes = 0;
  while (lexer->lookahead == '"') {
    take(lexer);
    quotes++;
  }
  if (lexer->lookahead != '[') return false;
  take(lexer);

  while (lexer->lookahead != 0 && lexer->lookahead != '\n') {
    if (lexer->lookahead != ']') {
      take(lexer);
      continue;
    }

    take(lexer);
    uint16_t closing_quotes = 0;
    while (lexer->lookahead == '"' && closing_quotes < quotes) {
      take(lexer);
      closing_quotes++;
    }
    if (closing_quotes == quotes) {
      lexer->mark_end(lexer);
      lexer->result_symbol = INLINE_VERBATIM_TOKEN;
      return true;
    }
  }

  return false;
}

static bool backtick_starts_inline(TSLexer *lexer) {
  take(lexer);
  if (lexer->lookahead == '`') return true;
  if (lexer->lookahead == '[') return true;

  bool has_kind = false;
  if (lexer->lookahead == '"') {
    while (lexer->lookahead == '"') take(lexer);
    return lexer->lookahead == '[';
  }

  while (lexer->lookahead != 0 && lexer->lookahead != '\n' &&
         lexer->lookahead != ' ' && lexer->lookahead != '\t' &&
         lexer->lookahead != '[' && lexer->lookahead != '{' &&
         lexer->lookahead != '`' && lexer->lookahead != '"') {
    take(lexer);
    has_kind = true;
  }
  return has_kind && lexer->lookahead == '[';
}

static bool scan_paragraph_continue(Scanner *scanner, TSLexer *lexer) {
  if (lexer->lookahead != '\n') return false;
  take(lexer);

  uint16_t column = 0;
  while (lexer->lookahead == ' ' && column < scanner->indents[scanner->depth]) {
    take(lexer);
    column++;
  }

  if (column != scanner->indents[scanner->depth] ||
      lexer->lookahead == ' ' || lexer->lookahead == '\n' ||
      lexer->lookahead == 0) {
    return false;
  }

  lexer->mark_end(lexer);
  if (lexer->lookahead == '`' && !backtick_starts_inline(lexer)) return false;
  lexer->result_symbol = PARAGRAPH_CONTINUE;
  return true;
}

static bool scan_inline_continue(Scanner *scanner, TSLexer *lexer) {
  if (lexer->lookahead != '\n') return false;
  take(lexer);

  uint16_t required = scanner->indents[scanner->depth];
  uint16_t column = 0;
  while (lexer->lookahead == ' ' && column < required) {
    take(lexer);
    column++;
  }
  if (column != required) return false;

  lexer->mark_end(lexer);
  while (lexer->lookahead == ' ') take(lexer);
  if (lexer->lookahead == '\n' || lexer->lookahead == 0) return false;
  if (lexer->lookahead == '`' && !backtick_starts_inline(lexer)) return false;

  lexer->result_symbol = INLINE_CONTINUE;
  return true;
}

static bool scan_layout(Scanner *scanner, TSLexer *lexer,
                        const bool *valid_symbols) {
  if (scanner->pending_dedents > 0 && valid_symbols[DEDENT]) {
    scanner->pending_dedents--;
    scanner->depth--;
    lexer->result_symbol = DEDENT;
    return true;
  }

  if (lexer->get_column(lexer) != 0) return false;
  lexer->mark_end(lexer);

  uint16_t column = 0;
  bool after_blank = false;
  for (;;) {
    while (lexer->lookahead == ' ') {
      skip(lexer);
      column++;
    }
    if (lexer->lookahead != '\n' || !valid_symbols[INDENT_AFTER_BLANK]) break;
    skip(lexer);
    column = 0;
    after_blank = true;
  }
  if (lexer->lookahead == '\n') return false;

  uint16_t current = scanner->indents[scanner->depth];
  if (lexer->lookahead == 0 && current > 0 && valid_symbols[DEDENT]) {
    scanner->depth--;
    lexer->result_symbol = DEDENT;
    return true;
  }
  if (lexer->lookahead == 0) return false;

  if (column == current && current > 0 && valid_symbols[SAME_INDENT]) {
    lexer->mark_end(lexer);
    lexer->result_symbol = SAME_INDENT;
    return true;
  }

  bool valid_indent = after_blank ? valid_symbols[INDENT_AFTER_BLANK]
                                  : valid_symbols[INDENT];
  if (column > current && valid_indent && scanner->depth + 1 < MAX_INDENT_DEPTH) {
    scanner->depth++;
    scanner->indents[scanner->depth] = column;
    lexer->mark_end(lexer);
    lexer->result_symbol = after_blank ? INDENT_AFTER_BLANK : INDENT;
    return true;
  }

  if (column < current && valid_symbols[DEDENT]) {
    uint8_t target = scanner->depth;
    while (target > 0 && scanner->indents[target] > column) target--;
    scanner->pending_dedents = scanner->depth - target - 1;
    scanner->depth--;
    lexer->result_symbol = DEDENT;
    return true;
  }

  return false;
}

bool tree_sitter_plumb_external_scanner_scan(void *payload, TSLexer *lexer,
                                              const bool *valid_symbols) {
  Scanner *scanner = payload;
  if (valid_symbols[INLINE_VERBATIM_TOKEN] && lexer->lookahead == '`') {
    return scan_inline_verbatim(lexer);
  }
  if (valid_symbols[RAW_CODE_LINE] && lexer->get_column(lexer) == 0) {
    return scan_raw_code_line(scanner, lexer, valid_symbols);
  }
  if (valid_symbols[PARAGRAPH_CONTINUE] && lexer->lookahead == '\n') {
    return scan_paragraph_continue(scanner, lexer);
  }
  if (valid_symbols[INLINE_CONTINUE] && lexer->lookahead == '\n') {
    return scan_inline_continue(scanner, lexer);
  }
  if (scan_layout(scanner, lexer, valid_symbols)) return true;
  if (valid_symbols[INCOMPLETE_INLINE_END] &&
      (lexer->lookahead == '\n' || lexer->lookahead == 0)) {
    lexer->result_symbol = INCOMPLETE_INLINE_END;
    return true;
  }
  if (valid_symbols[INCOMPLETE_ATTRIBUTES_END] &&
      (lexer->lookahead == '\n' || lexer->lookahead == 0)) {
    lexer->result_symbol = INCOMPLETE_ATTRIBUTES_END;
    return true;
  }
  if (valid_symbols[END_OF_FILE] && lexer->lookahead == 0) {
    lexer->result_symbol = END_OF_FILE;
    return true;
  }
  return false;
}
