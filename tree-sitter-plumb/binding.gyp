{
  "targets": [
    {
      "target_name": "tree_sitter_plumb_binding",
      "include_dirs": ["src"],
      "sources": [
        "bindings/node/binding.cc",
        "src/parser.c"
      ],
      "cflags_c": ["-std=c11"]
    }
  ]
}
