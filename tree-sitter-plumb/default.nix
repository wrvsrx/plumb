{
  lib,
  nodejs,
  stdenvNoCC,
  tree-sitter,
}:

let
  version = (builtins.fromJSON (builtins.readFile ./tree-sitter.json)).metadata.version;

  source = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./grammar.js
      ./tree-sitter.json
      ./queries
      ./src/scanner.c
    ];
  };

  generatedSource = stdenvNoCC.mkDerivation {
    pname = "tree-sitter-plumb-src";
    inherit version;
    src = source;

    nativeBuildInputs = [
      nodejs
      tree-sitter
    ];

    buildPhase = ''
      runHook preBuild
      tree-sitter generate
      runHook postBuild
    '';

    installPhase = ''
      runHook preInstall
      cp -r . "$out"
      runHook postInstall
    '';
  };
in
tree-sitter.buildGrammar {
  language = "plumb";
  inherit version;
  src = generatedSource;

  passthru.generatedSource = generatedSource;

  meta = {
    description = "Tree-sitter grammar for plumb";
    license = lib.licenses.mit;
  };
}
