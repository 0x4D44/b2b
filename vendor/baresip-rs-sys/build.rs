use std::{env, fs, path::PathBuf};

fn main() {
    // Try to link via pkg-config first when not vendoring.
    let vendored = cfg!(feature = "vendored");
    let static_link = cfg!(feature = "static-link");

    if !vendored {
        if try_pkg_config(static_link) {
            maybe_generate_bindings(None);
            return;
        }
        // Hard error: do not silently attempt vendored builds unless the
        // `vendored` feature is explicitly enabled. Provide actionable guidance.
        panic!(
            "\nFailed to locate libre/baresip via pkg-config.\n\n  Hint:\n    - Install development packages that provide `libre.pc` and `libbaresip.pc`.\n      * Ubuntu/Debian: libbaresip-dev libre-dev (names may vary)\n      * Arch Linux: community/baresip (includes libre)\n    - Or enable the vendored build: `--features vendored` and point to sources with\n      RE_SRC_DIR=/path/to/re  BARESIP_SRC_DIR=/path/to/baresip\n      (No network fetch is performed in build.rs; sources must be present locally.)\n\n  If you intended a fully static build, also consider `--features static-link` and\n  set PKG_CONFIG_ALL_STATIC=1 as supported by your system.\n"
        );
    }

    // Vendored path: build re (libre) then baresip via CMake, install into OUT_DIR/prefix
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let prefix_hint = out_dir.join("prefix");
    let _ = fs::create_dir_all(&prefix_hint);

    // Locate sources for vendored builds:
    // - Prefer repo-local third_party/src when building from the Git repo.
    // - Alternatively, honor RE_SRC_DIR and BARESIP_SRC_DIR env vars.
    // - Do NOT fetch sources from the network in build.rs (crates.io policy).
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let repo_src = manifest_dir.join("..").join("third_party").join("src");
    let src_dir = if repo_src.join("re").exists() && repo_src.join("baresip").exists() {
        repo_src
    } else if let (Ok(re_dir), Ok(bs_dir)) = (env::var("RE_SRC_DIR"), env::var("BARESIP_SRC_DIR")) {
        // Symlink or copy into an OUT_DIR location if needed
        let root = out_dir.join("src_vendor");
        let _ = fs::create_dir_all(&root);
        build_from_paths(PathBuf::from(re_dir), PathBuf::from(bs_dir), &root, static_link);
        return;
    } else {
        panic!("vendored build requested but sources not found. Set RE_SRC_DIR and BARESIP_SRC_DIR or build from the Git repo with third_party/src available.");
    };

    // Build libre (re)
    let re_build = cmake::Config::new(src_dir.join("re"))
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        .define("CMAKE_INSTALL_PREFIX", &prefix_hint)
        .profile("Release")
        .build();
    
    // Build baresip
    let mut bs_cfg = cmake::Config::new(src_dir.join("baresip"));
    bs_cfg
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        .define("CMAKE_INSTALL_PREFIX", &prefix_hint)
        .define("STATIC", "ON")
        .profile("Release");
    if let Ok(mods) = env::var("BARESIP_MODULES") {
        bs_cfg.define("MODULES", &mods);
        println!("cargo:warning=baresip modules (from env): {}", mods);
    } else {
        // Enable safe, no-extra-deps audio codecs by default.
        let mut mods = String::from("g711;l16;g722");
        // Opportunistically enable opus if available on the system.
        let opus_ok = pkg_config::Config::new().probe("opus").is_ok();
        if opus_ok {
            mods.push_str(";opus");
            println!("cargo:warning=libopus detected via pkg-config; enabling opus module");
        } else {
            println!("cargo:warning=libopus not found; opus module disabled");
        }
        bs_cfg.define("MODULES", &mods);
        println!("cargo:warning=baresip modules (auto): {}", mods);
    }
    let bs_build = bs_cfg.build();
    
    // After install, add link search path and explicit static libs for vendored build.
    let lib_dir = prefix_hint.join("lib");
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static=baresip");
    println!("cargo:rustc-link-lib=static=re");
    // Ensure transitive deps resolve on most systems when pkg-config metadata
    // is incomplete or unavailable at runtime.
    println!("cargo:rustc-link-lib=dylib=z");

    // Also probe with pkg-config to capture transitive link flags (openssl, zlib, srtp, etc.).
    let pc_dir = find_pc_dir(&re_build).or_else(|| find_pc_dir(&bs_build)).expect("pkgconfig dir not found after build");
    let old_pc = env::var("PKG_CONFIG_PATH").unwrap_or_default();
    let new_pc = if old_pc.is_empty() { pc_dir.display().to_string() } else { format!("{}:{}", pc_dir.display(), old_pc) };
    unsafe { env::set_var("PKG_CONFIG_PATH", &new_pc) };

    let _ = try_pkg_config(static_link);

    // Generate bindings if enabled, using installed headers under prefix/include
    // Try common include roots produced by CMake install
    let include_dir = re_build.join("include");
    let include_dir = if include_dir.exists() { include_dir } else { bs_build.join("include") };
    maybe_generate_bindings(Some(include_dir));
}

fn try_pkg_config(static_link: bool) -> bool {
    let mut cfg = pkg_config::Config::new();
    if static_link {
        cfg.statik(true);
        // allow fallback if static not available
        cfg.print_system_cflags(true);
    }
    // Probe libre first (aka re)
    let re_ok = cfg.clone().probe("libre").or_else(|_| cfg.clone().probe("re")).is_ok();
    if !re_ok { return false; }

    // Probe baresip
    let bs_ok = cfg.clone().atleast_version("4.0").probe("libbaresip").is_ok();
    if !bs_ok { return false; }

    true
}

fn find_pc_dir(prefix: &std::path::Path) -> Option<PathBuf> {
    let p1 = prefix.join("lib").join("pkgconfig");
    if p1.exists() { return Some(p1); }
    let p2 = prefix.join("prefix").join("lib").join("pkgconfig");
    if p2.exists() { return Some(p2); }
    None
}

#[allow(dead_code)]
fn build_from_paths(re_src: PathBuf, bs_src: PathBuf, _root: &PathBuf, static_link: bool) {
    // Build libre
    let re_build = cmake::Config::new(re_src)
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        .profile("Release")
        .build();
    // Build baresip
    let mut bs_cfg = cmake::Config::new(bs_src);
    bs_cfg
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        .define("STATIC", "ON")
        .profile("Release");
    if let Ok(mods) = env::var("BARESIP_MODULES") { bs_cfg.define("MODULES", &mods); } else { bs_cfg.define("MODULES", ""); }
    let bs_build = bs_cfg.build();
    // Link search + libs
    let lib_dir = bs_build.join("lib");
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static=baresip");
    println!("cargo:rustc-link-lib=static=re");
    println!("cargo:rustc-link-lib=dylib=z");
    // Try pkg-config for transitive flags
    if let Some(pc_dir) = find_pc_dir(&re_build).or_else(|| find_pc_dir(&bs_build)) {
        let old_pc = env::var("PKG_CONFIG_PATH").unwrap_or_default();
        let new_pc = if old_pc.is_empty() { pc_dir.display().to_string() } else { format!("{}:{}", pc_dir.display(), old_pc) };
        unsafe { env::set_var("PKG_CONFIG_PATH", &new_pc) };
        let _ = try_pkg_config(static_link);
    }
    // Bindings (optional)
    maybe_generate_bindings(None);
    // Re-run pkg-config to emit transitive link flags
}

#[cfg(feature = "bindgen")]
fn maybe_generate_bindings(optional_include: Option<PathBuf>) {
    let mut builder = bindgen::Builder::default()
        .allowlist_file(".*/baresip\\.h")
        .allowlist_file(".*/re/.*\\.h")
        .allowlist_type("^(re_|baresip_|ua_|call_).*")
        .allowlist_function("^(re_|baresip_|ua_|call_).*")
        .header_contents("wrapper.h", "#include <re/re.h>\n#include <baresip.h>\n")
        .clang_arg("-D__STDC_CONSTANT_MACROS")
        .clang_arg("-D__STDC_LIMIT_MACROS")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()));

    if let Some(inc) = optional_include {
        builder = builder.clang_arg(format!("-I{}", inc.display()));
    }
    if let Ok(inc) = env::var("RE_INCLUDE_DIR") { builder = builder.clang_arg(format!("-I{}", inc)); }
    if let Ok(inc) = env::var("BARESIP_INCLUDE_DIR") { builder = builder.clang_arg(format!("-I{}", inc)); }

    let bindings = builder.generate().expect("bindgen failed");
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("bindings.rs");
    bindings.write_to_file(&out_path).unwrap();
    println!("cargo:rerun-if-changed=build.rs");
}

#[cfg(not(feature = "bindgen"))]
fn maybe_generate_bindings(_optional_include: Option<PathBuf>) {}
