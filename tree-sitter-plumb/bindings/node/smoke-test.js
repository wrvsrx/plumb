const assert = require('node:assert/strict');
const Parser = require('tree-sitter');
const Plumb = require('./index');

const parser = new Parser();
parser.setLanguage(Plumb);
const tree = parser.parse('`# heading\n');

assert.equal(tree.rootNode.type, 'document');
assert.equal(tree.rootNode.hasError, false);
