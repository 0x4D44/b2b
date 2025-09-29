fn main() {
    // Include headers from vendored sources for convenience, but the shim here
    // is currently a no-op stub that does not depend on them yet. Keep them in
    // place so we can flesh out the implementation without changing build glue.
    println!("cargo:rerun-if-changed=c/sip_shim.c");
    println!("cargo:rerun-if-changed=third_party/src/re/include");
    println!("cargo:rerun-if-changed=third_party/src/baresip/include");

    let mut build = cc::Build::new();
    build.file("c/sip_shim.c");
    build.include("third_party/src/re/include");
    build.include("third_party/src/baresip/include");
    build.warnings(false);
    build.compile("b2b_sip_shim");

    // Ensure common system libs are appended at the end of the link line
    // so order does not break resolution against static libs.
    println!("cargo:rustc-link-arg=-lz");
    println!("cargo:rustc-link-arg=-lssl");
    println!("cargo:rustc-link-arg=-lcrypto");
}
