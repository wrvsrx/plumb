// plumb external scanner.
//
// Locality budget (core syntax reference §2): scanner state carries only the
// indentation stack and the verbatim margin. Inline token recognition stays
// on one physical line; raw blank runs look ahead to the next nonblank line so
// internal blank payload and trailing block layout remain distinguishable.

#include "tree_sitter/parser.h"

#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

enum TokenType {
  INDENT,
  SAME_LINE_CHILD_INDENT,
  INDENT_AFTER_BLANK,
  SAME_INDENT,
  PARAGRAPH_CONTINUE,
  INLINE_CONTINUE,
  DEDENT,
  VERBATIM_BLOCK_OPEN,
  RAW_TAIL_OPEN,
  RAW_CODE_LINE,
  INLINE_VERBATIM_TOKEN,
  INLINE_CHILD_KIND,
  INCOMPLETE_INLINE_END,
  END_OF_FILE,
};

enum BacktickDispatch {
  BACKTICK_ESCAPE,
  BACKTICK_PARSED_INLINE,
  BACKTICK_INLINE_VERBATIM,
  BACKTICK_MARKED_BLOCK,
  BACKTICK_VERBATIM_BLOCK,
};

#define MAX_INDENT_DEPTH 64

typedef struct {
  uint16_t indents[MAX_INDENT_DEPTH];
  uint16_t verbatim_margin;
  uint8_t depth;
  uint8_t pending_dedents;
} Scanner;

static void skip(TSLexer *lexer) { lexer->advance(lexer, true); }
static void take(TSLexer *lexer) { lexer->advance(lexer, false); }

void *tree_sitter_plumb_external_scanner_create(void) {
  Scanner *scanner = calloc(1, sizeof(Scanner));
  scanner->verbatim_margin = 1;
  return scanner;
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
  buffer[size++] = (char)(scanner->verbatim_margin & 0xff);
  buffer[size++] = (char)(scanner->verbatim_margin >> 8);
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
  scanner->verbatim_margin = 1;
  scanner->indents[0] = 0;
  if (length < 4) return;

  scanner->depth = (uint8_t)buffer[0];
  scanner->pending_dedents = (uint8_t)buffer[1];
  scanner->verbatim_margin = (uint8_t)buffer[2] |
                             ((uint16_t)(uint8_t)buffer[3] << 8);
  if (scanner->depth >= MAX_INDENT_DEPTH) scanner->depth = MAX_INDENT_DEPTH - 1;
  scanner->pending_dedents = (uint8_t)buffer[1];
  unsigned available = (length - 4) / 2;
  if (available <= scanner->depth) scanner->depth = available ? available - 1 : 0;
  for (uint8_t i = 0; i <= scanner->depth; i++) {
    unsigned offset = 4u + (unsigned)i * 2u;
    if (offset + 1u >= length) break;
    scanner->indents[i] = (uint8_t)buffer[offset] |
                          ((uint16_t)(uint8_t)buffer[offset + 1u] << 8);
  }
}

static bool scan_raw_code_line(Scanner *scanner, TSLexer *lexer,
                               const bool *valid_symbols) {
  lexer->mark_end(lexer);
  uint32_t verbatim_indent = (uint32_t)scanner->indents[scanner->depth] +
                             scanner->verbatim_margin;
  uint32_t spaces = 0;
  while (lexer->lookahead == ' ' && spaces < verbatim_indent) {
    skip(lexer);
    spaces++;
  }

  if (lexer->lookahead == '\n') {
    if (spaces >= verbatim_indent) {
      take(lexer);
      lexer->mark_end(lexer);
      lexer->result_symbol = RAW_CODE_LINE;
      return true;
    }
    if (spaces > 0) return false;

    for (;;) {
      take(lexer);
      uint32_t next_spaces = 0;
      while (lexer->lookahead == ' ' && next_spaces < verbatim_indent) {
        take(lexer);
        next_spaces++;
      }
      if (lexer->lookahead == '\n') continue;
      if (lexer->lookahead == 0 || next_spaces < verbatim_indent) return false;
      while (lexer->lookahead != '\n' && lexer->lookahead != 0) take(lexer);
      if (lexer->lookahead == '\n') take(lexer);
      lexer->mark_end(lexer);
      lexer->result_symbol = RAW_CODE_LINE;
      return true;
    }
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

static uint16_t scan_quote_run(TSLexer *lexer) {
  uint16_t quotes = 0;
  while (lexer->lookahead == '"') {
    take(lexer);
    if (quotes < UINT16_MAX) quotes++;
  }
  return quotes;
}

static bool scan_strengthened_close(TSLexer *lexer, uint16_t quotes) {
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
      return true;
    }
  }

  return false;
}

static bool scan_verbatim_close(TSLexer *lexer, uint16_t quotes) {
  if (lexer->lookahead == '[') {
    return scan_strengthened_close(lexer, quotes);
  }
  if (quotes != 1 || lexer->lookahead == '\n' || lexer->lookahead == 0) {
    return false;
  }
  while (lexer->lookahead != 0 && lexer->lookahead != '\n') {
    if (lexer->lookahead == '"') {
      take(lexer);
      return true;
    }
    take(lexer);
  }
  return false;
}

// Dispatch lookahead may advance the lexer, but must not mark or emit a token.
static enum BacktickDispatch classify_verbatim_after_open(TSLexer *lexer,
                                                          uint16_t quotes) {
  if (lexer->lookahead == '[') {
    return scan_strengthened_close(lexer, quotes) ? BACKTICK_INLINE_VERBATIM
                                                   : BACKTICK_VERBATIM_BLOCK;
  }
  if (quotes != 1 || lexer->lookahead == '\n' || lexer->lookahead == 0) {
    return BACKTICK_VERBATIM_BLOCK;
  }

  while (lexer->lookahead != 0 && lexer->lookahead != '\n') {
    if (lexer->lookahead == '"') {
      take(lexer);
      return BACKTICK_INLINE_VERBATIM;
    }
    take(lexer);
  }
  return BACKTICK_VERBATIM_BLOCK;
}

static bool is_name_char(int32_t character) {
  return character != 0 && character != ' ' && character != '\t' &&
         character != '\n' && character != '\r' && character != '[' &&
         character != ']' && character != '`' && character != '"' &&
         character != '|' &&
         !(character >= 0x01 && character <= 0x1f) &&
         !(character >= 0x7f && character <= 0x9f);
}

static enum BacktickDispatch classify_backtick_dispatch(TSLexer *lexer) {
  take(lexer);
  if (lexer->lookahead == '`' || lexer->lookahead == '[' ||
      lexer->lookahead == ']' || lexer->lookahead == '|') {
    return BACKTICK_ESCAPE;
  }

  if (lexer->lookahead == '"') {
    uint16_t quotes = scan_quote_run(lexer);
    return classify_verbatim_after_open(lexer, quotes);
  }

  bool has_kind = false;
  while (is_name_char(lexer->lookahead)) {
    take(lexer);
    has_kind = true;
  }
  if (has_kind && lexer->lookahead == '[') return BACKTICK_PARSED_INLINE;
  if (lexer->lookahead == '"') {
    uint16_t quotes = scan_quote_run(lexer);
    return scan_verbatim_close(lexer, quotes) ? BACKTICK_INLINE_VERBATIM
                                               : BACKTICK_MARKED_BLOCK;
  }
  return BACKTICK_MARKED_BLOCK;
}

static bool is_inline_dispatch(enum BacktickDispatch dispatch) {
  return dispatch == BACKTICK_ESCAPE || dispatch == BACKTICK_PARSED_INLINE ||
         dispatch == BACKTICK_INLINE_VERBATIM;
}

static bool scan_verbatim_block_open(Scanner *scanner, TSLexer *lexer,
                                     const bool *valid_symbols) {
  if (lexer->lookahead != '"') return false;

  uint16_t quotes = scan_quote_run(lexer);
  lexer->mark_end(lexer);
  enum BacktickDispatch dispatch =
      classify_verbatim_after_open(lexer, quotes);
  if (dispatch == BACKTICK_INLINE_VERBATIM) {
    lexer->mark_end(lexer);
    lexer->result_symbol = INLINE_VERBATIM_TOKEN;
    return valid_symbols[INLINE_VERBATIM_TOKEN];
  }

  scanner->verbatim_margin = quotes;
  lexer->result_symbol = VERBATIM_BLOCK_OPEN;
  return true;
}

static bool scan_raw_tail_open(Scanner *scanner, TSLexer *lexer) {
  if (lexer->lookahead != '"') return false;
  take(lexer);
  lexer->mark_end(lexer);
  if (lexer->lookahead != '\n' && lexer->lookahead != '\r' &&
      lexer->lookahead != 0) {
    return false;
  }
  scanner->verbatim_margin = 1;
  lexer->result_symbol = RAW_TAIL_OPEN;
  return true;
}

static bool scan_inline_verbatim_body(TSLexer *lexer) {
  if (lexer->lookahead != '"') return false;
  uint16_t quotes = scan_quote_run(lexer);
  if (!scan_verbatim_close(lexer, quotes)) return false;
  lexer->mark_end(lexer);
  lexer->result_symbol = INLINE_VERBATIM_TOKEN;
  return true;
}

static bool scan_inline_child_kind(TSLexer *lexer) {
  if (!is_name_char(lexer->lookahead)) return false;
  do {
    take(lexer);
  } while (is_name_char(lexer->lookahead));
  if (lexer->lookahead != '[' && lexer->lookahead != '"') return false;
  lexer->mark_end(lexer);
  lexer->result_symbol = INLINE_CHILD_KIND;
  return true;
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
  if (lexer->lookahead == '`') {
    enum BacktickDispatch dispatch = classify_backtick_dispatch(lexer);
    if (!is_inline_dispatch(dispatch)) return false;
  }
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
  if (lexer->lookahead == '`') {
    enum BacktickDispatch dispatch = classify_backtick_dispatch(lexer);
    if (!is_inline_dispatch(dispatch)) return false;
  }

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

  if (lexer->lookahead == 0 && scanner->depth > 0 && valid_symbols[DEDENT]) {
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
    bool can_scan_blank =
        valid_symbols[INDENT_AFTER_BLANK] || valid_symbols[DEDENT] ||
        valid_symbols[RAW_TAIL_OPEN];
    if (!can_scan_blank) break;
    if (lexer->lookahead == '\r') skip(lexer);
    if (lexer->lookahead != '\n') {
      break;
    }
    skip(lexer);
    column = 0;
    after_blank = true;
  }
  if (lexer->lookahead == '\n' || lexer->lookahead == '\r') return false;

  uint16_t current = scanner->indents[scanner->depth];
  if (valid_symbols[RAW_TAIL_OPEN] && column == current &&
      lexer->lookahead == '"') {
    if (!scan_raw_tail_open(scanner, lexer)) return false;
    return true;
  }
  if (after_blank && !valid_symbols[INDENT_AFTER_BLANK] && column >= current) {
    return false;
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

static bool scan_same_line_child_indent(Scanner *scanner, TSLexer *lexer) {
  uint16_t column = lexer->get_column(lexer);
  uint16_t current = scanner->indents[scanner->depth];
  if (column <= current || scanner->depth + 1 >= MAX_INDENT_DEPTH) return false;
  if (lexer->lookahead != '`') return false;
  lexer->mark_end(lexer);
  enum BacktickDispatch dispatch = classify_backtick_dispatch(lexer);
  if (is_inline_dispatch(dispatch)) return false;

  scanner->depth++;
  scanner->indents[scanner->depth] = column;
  lexer->result_symbol = SAME_LINE_CHILD_INDENT;
  return true;
}

bool tree_sitter_plumb_external_scanner_scan(void *payload, TSLexer *lexer,
                                              const bool *valid_symbols) {
  Scanner *scanner = payload;
  uint16_t column = lexer->get_column(lexer);
  if (valid_symbols[SAME_LINE_CHILD_INDENT] && lexer->lookahead == '`' &&
      column > scanner->indents[scanner->depth] &&
      scanner->depth + 1 < MAX_INDENT_DEPTH) {
    return scan_same_line_child_indent(scanner, lexer);
  }
  if (valid_symbols[VERBATIM_BLOCK_OPEN] && lexer->lookahead == '"') {
    return scan_verbatim_block_open(scanner, lexer, valid_symbols);
  }
  if (valid_symbols[RAW_TAIL_OPEN] && lexer->lookahead == '"' &&
      lexer->get_column(lexer) == scanner->indents[scanner->depth]) {
    return scan_raw_tail_open(scanner, lexer);
  }
  if (valid_symbols[INLINE_VERBATIM_TOKEN] && lexer->lookahead == '"') {
    return scan_inline_verbatim_body(lexer);
  }
  if (valid_symbols[INLINE_CHILD_KIND]) {
    return scan_inline_child_kind(lexer);
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
  if (valid_symbols[END_OF_FILE] && lexer->lookahead == 0) {
    lexer->result_symbol = END_OF_FILE;
    return true;
  }
  return false;
}
