fn main() {
    println!("cargo:rerun-if-changed=src/libffisan/lib.c");
    cc::Build::new()
        .file("src/libffisan/lib.c")
        .flag("-std=c11")
        .flag("-fPIC")
        .warnings(true)
        .extra_warnings(true)
        .debug(true)
        .compile("ffisan");
    println!("cargo:rustc-link-lib=static=ffisan");
}
