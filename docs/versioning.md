# Version update guide

Update every version source together:

1. Set the release version in `Cargo.toml` and let Cargo update `Cargo.lock`.
2. Set the same version in `contrib/nvim/lua/plumb/version.lua` and
   `tree-sitter-plumb/tree-sitter.json`.
3. Run `cargo check --workspace --all-targets` and the relevant tests.
4. Commit the release, create a tag matching the version, then bump all version
   sources to the next `-dev` version in a separate commit.

The release tag must contain matching versions in the binary, bundled Neovim
plugin, and tree-sitter package. Do not move or delete an existing release tag;
if a release is wrong, publish the next patch version.
