{
  description = "Strict plumb markup language and tooling";

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
            cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
            tree-sitter-plumb = pkgs.callPackage ./tree-sitter-plumb/default.nix { };
            plumb = pkgs.rustPlatform.buildRustPackage {
              pname = "plumb";
              version = cargoToml.workspace.package.version;
              src = pkgs.lib.cleanSource ./.;
              cargoLock.lockFile = ./Cargo.lock;

              postInstall = ''
                mkdir -p $out/share/plumb
                cp -r skills $out/share/plumb/
                cp -r contrib $out/share/plumb/
              '';

              passthru = {
                "tree-sitter-plumb" = tree-sitter-plumb;
              };

              meta = {
                description = "Strict plumb markup language and tooling";
                license = pkgs.lib.licenses.mit;
                mainProgram = "plumb";
              };
            };
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
            packages = {
              inherit plumb;
              default = plumb;
            };
            formatter = pkgs.nixfmt;
          };
      }
    );
}
