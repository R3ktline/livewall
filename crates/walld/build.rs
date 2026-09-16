fn main() {
    let mut build = cc::Build::new();
    build.file("native/wall_decode.c");
    build.include("native");

    for lib in ["libavformat", "libavcodec", "libavutil", "libswscale"] {
        let lib = pkg_config::probe_library(lib).unwrap_or_else(|e| panic!("{lib}: {e}"));
        for p in &lib.include_paths {
            build.include(p);
        }
        for p in &lib.link_paths {
            println!("cargo:rustc-link-search=native={}", p.display());
        }
        for name in &lib.libs {
            println!("cargo:rustc-link-lib={}", name);
        }
    }

    build.warnings(false);
    build.compile("wall_decode");

    println!("cargo:rerun-if-changed=native/wall_decode.c");
    println!("cargo:rerun-if-changed=native/wall_decode.h");
}
