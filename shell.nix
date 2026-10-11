{ cargo, clippy, mkShell, rustc, rustfmt }:
mkShell {
  # Avoid injecting a self-rpath that breaks linking in worktrees with spaces.
  env.NIX_NO_SELF_RPATH = true;

  packages = [
    cargo
    clippy
    rustc
    rustfmt
  ];
}
