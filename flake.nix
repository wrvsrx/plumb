{
  description = "flake template";

  inputs = {
    nixpkgs.url = "github:wrvsrx/nixpkgs/patched-nixos-unstable";
    flake-parts = {
      url = "github:hercules-ci/flake-parts";
      inputs.nixpkgs-lib.follows = "nixpkgs";
    };
  };

  outputs =
    inputs:
    inputs.flake-parts.lib.mkFlake { inherit inputs; } (
      { inputs, ... }:
      {
        systems = [ "x86_64-linux" ];
        perSystem =
          { pkgs, ... }:
          let
            tree-sitter-plumb = pkgs.callPackage ./tree-sitter-plumb/default.nix { };
          in
          {
            devShells.default = pkgs.callPackage ./shell.nix { };
            devShells.tree-sitter-plumb = pkgs.mkShell {
              inputsFrom = [
                tree-sitter-plumb
                tree-sitter-plumb.generatedSource
              ];

              shellHook = ''
                export CC="${pkgs.stdenv.cc}/bin/cc"
              '';
            };
            packages = { inherit tree-sitter-plumb; };
            formatter = pkgs.nixfmt;
          };
      }
    );
}
