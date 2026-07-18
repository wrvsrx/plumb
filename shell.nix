{ cargo, clippy, mkShell, rustc, rustfmt }:
mkShell {
  packages = [
    cargo
    clippy
    rustc
    rustfmt
  ];
}
