use std::path::{Path, PathBuf};

fn get_git_version() -> Result<String, Box<dyn std::error::Error>> {
    if Path::new(".git/HEAD").exists() {
        println!("cargo:rerun-if-changed=.git/HEAD");
        let r = std::fs::read_to_string(".git/HEAD").unwrap();
        let mut r = r.split_ascii_whitespace();
        r.next();
        let branch = r.next().unwrap();
        println!("cargo:rerun-if-changed=.git/{}", branch);
    }
    let git_version = std::process::Command::new("git")
        .arg("describe")
        .arg("--dirty")
        .output()?;
    let mut git_version = String::from_utf8(git_version.stdout)?;
    git_version.pop();
    let full_version = git_version.strip_prefix('v').ok_or("error")?.to_owned();
    Ok(full_version)
}
fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let full_version =
        get_git_version().unwrap_or_else(|_| std::env::var("CARGO_PKG_VERSION").unwrap());

    assert!(
        full_version.starts_with(&std::env::var("CARGO_PKG_VERSION").unwrap()),
        "latest git tag does not match the version set in cargo: {} vs {}",
        full_version,
        std::env::var("CARGO_PKG_VERSION").unwrap()
    );

    println!(
        "cargo:rustc-env=XASKPASS_BUILD_FULL_VERSION={}",
        full_version
    );

    let out_path = match std::env::var_os("XASKPASS_BUILDDIR") {
        Some(path) => std::fs::canonicalize(path).unwrap(),
        None => match std::fs::canonicalize("pregen") {
            Err(_) => PathBuf::from(std::env::var_os("OUT_DIR").unwrap()),
            Ok(path) => path,
        },
    };

    let mut man = std::fs::read_to_string("xaskpass.man.in").unwrap();
    man = man.replace("{VERSION}", &full_version);
    std::fs::write(out_path.join("xaskpass.man"), man).unwrap();

    let deps = [
        (
            &[("xkbcommon", "0.10"), ("xkbcommon-x11", "0.10")] as &[(&str, &str)],
            &[
                ("src/keyboard/ffi.h", "xkbcommon.rs", "xkb_.*|XKB_.*"),
                ("src/keyboard/ffi_names.h", "xkbcommon-names.rs", ".*"),
                ("src/keyboard/ffi_keysyms.h", "xkbcommon-keysyms.rs", ".*"),
            ] as &[(&str, &str, &str)],
        ),
        (
            &[("pango", "1.30")],
            &[(
                "src/dialog/pango_sys_fixes.h",
                "pango_sys_fixes.rs",
                "PangoLayoutLine|PangoLogAttr",
            )],
        ),
    ];

    println!(
        "cargo:rustc-env=XASKPASS_BUILD_HEADER_DIR={}",
        out_path.display()
    );

    for (libs, headers) in deps {
        let mut include_paths = vec![];
        for (dep, version) in libs {
            match pkg_config::Config::new()
                .atleast_version(version)
                .probe(dep)
            {
                Err(s) => {
                    eprintln!("{}", s);
                    std::process::exit(1);
                }
                Ok(lib) => include_paths.extend(lib.include_paths),
            }
        }

        for (header, out, whitelist) in headers {
            let out = out_path.join(out);
            if out.exists() {
                continue;
            }
            println!("cargo:rerun-if-changed={}", header);
            println!("cargo:rerun-if-changed={}", out.display());

            let mut bindgen = bindgen::Builder::default()
                .header(*header)
                .parse_callbacks(Box::new(bindgen::CargoCallbacks))
                .allowlist_function(whitelist)
                .allowlist_type(whitelist)
                .allowlist_var(whitelist)
                .default_enum_style(bindgen::EnumVariation::ModuleConsts);
            for inc in &include_paths {
                bindgen = bindgen
                    .clang_arg("-I")
                    .clang_arg(inc.as_os_str().to_string_lossy());
            }
            let bindings = bindgen.generate().expect("Unable to generate bindings");
            bindings
                .write_to_file(out)
                .expect("Couldn't write bindings!");
        }
    }
}
