fn main() {
    let grammar = "../../src/parser.c";
    let scanner = "../../src/scanner.c";
    assert!(
        std::path::Path::new(grammar).exists(),
        "run tree-sitter generate before the strict alignment test"
    );

    cc::Build::new()
        .include("../../src")
        .file(grammar)
        .file(scanner)
        .opt_level(1)
        .flag_if_supported("-std=c11")
        .compile("tree-sitter-plumb");

    println!("cargo:rerun-if-changed={grammar}");
    println!("cargo:rerun-if-changed={scanner}");
}
