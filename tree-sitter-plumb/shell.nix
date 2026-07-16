{
  mkShell,
  stdenv,
  tree-sitter,
}:
mkShell {
  packages = [
    stdenv.cc
    tree-sitter
  ];

  shellHook = ''
    export CC="${stdenv.cc}/bin/cc"
  '';
}
