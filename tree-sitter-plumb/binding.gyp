{
  "targets": [
    {
      "target_name": "tree_sitter_plumb_binding",
      "include_dirs": [
        "src",
        "<!@(node -p \"require('node-addon-api').include\")"
      ],
      "dependencies": ["<!(node -p \"require('node-addon-api').gyp\")"],
      "defines": [
        "NAPI_VERSION=8",
        "NODE_ADDON_API_DISABLE_CPP_EXCEPTIONS"
      ],
      "sources": [
        "bindings/node/binding.cc",
        "src/parser.c",
        "src/scanner.c"
      ],
      "cflags_c": ["-std=c11"]
    }
  ]
}
