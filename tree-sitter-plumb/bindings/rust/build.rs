fn main() {
    let src = std::path::Path::new("src");
    println!("cargo:rerun-if-changed={}", src.join("parser.c").display());
    println!("cargo:rerun-if-changed={}", src.join("scanner.c").display());

    cc::Build::new()
        .include(src)
        .file(src.join("parser.c"))
        .file(src.join("scanner.c"))
        .warnings(false)
        .compile("tree-sitter-plumb");
}
