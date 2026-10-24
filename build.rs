fn main() {
    assert_eq!(
        std::env::var("CARGO_CFG_TARGET_OS").as_deref(),
        Ok("linux"),
        "Likh currently supports Linux only"
    );
    let xft =
        pkg_config::probe_library("xft").expect("Install libxft-dev and pkg-config to build Likh");
    let mut native = cc::Build::new();
    for path in xft.include_paths {
        native.include(path);
    }
    native
        .file("native/desktop.c")
        .warnings(true)
        .flag("-std=c11")
        .compile("likh_desktop");
    println!("cargo:rustc-link-lib=X11");
    println!("cargo:rustc-link-lib=Xext");
    println!("cargo:rustc-link-lib=Xrandr");
    println!("cargo:rustc-link-lib=dylib:+verbatim=libXtst.so.6");
    println!("cargo:rerun-if-changed=native/desktop.c");
    println!("cargo:rerun-if-changed=native/scale.h");
}
