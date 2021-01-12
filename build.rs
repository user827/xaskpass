use std::fs::File;
use std::io::BufReader;
use std::io::Write as _;
use std::path::PathBuf;

fn get_git_version() -> Result<String, Box<dyn std::error::Error>> {
    let git_version = std::process::Command::new("git").arg("describe").output()?;
    let mut git_version = String::from_utf8(git_version.stdout)?;
    git_version.pop();
    let full_version = git_version.strip_prefix("v").ok_or("error")?.to_owned();
    Ok(full_version)
}
fn main() {
    let full_version =
        get_git_version().unwrap_or_else(|_| std::env::var("CARGO_PKG_VERSION").unwrap());

    // commenting this out as it would make the life hard for linters
    //assert!(
    //    full_version.starts_with(&std::env::var("CARGO_PKG_VERSION").unwrap()),
    //    "latest git tag does not match the version set in cargo"
    //);

    println!(
        "cargo:rustc-env=XASKPASS_BUILD_FULL_VERSION={}",
        full_version
    );

    let deps = [("xkbcommon", "0.10"), ("xkbcommon-x11", "0.10")];

    let use_xcb_errors = pkg_config::Config::new()
        .atleast_version("1.0")
        .probe("xcb-errors")
        .is_ok();
    if use_xcb_errors {
        println!("cargo:rustc-cfg=xcb_errors");
    }

    for (dep, version) in &deps {
        if let Err(s) = pkg_config::Config::new()
            .atleast_version(version)
            .probe(dep)
        {
            eprintln!("{}", s);
            std::process::exit(1);
        }
    }

    let mut headers = vec![
        ("src/keyboard/ffi.h", "xkbcommon.rs", "xkb_.*|XKB_.*"),
        ("src/keyboard/ffi_names.h", "xkbcommon-names.rs", ".*"),
        ("src/keyboard/ffi_keysyms.h", "xkbcommon-keysyms.rs", ".*"),
    ];
    if use_xcb_errors {
        headers.push((
            "src/errors/xcb_errors/ffi.h",
            "xcb-errors.rs",
            "xcb_errors_.*",
        ));
    }

    // Because clippy is so slow otherwise
    let out_path = match std::fs::canonicalize("pregen") {
        Err(_) => PathBuf::from(std::env::var("OUT_DIR").unwrap()),
        Ok(path) => path,
    };
    println!(
        "cargo:rustc-env=XASKPASS_BUILD_HEADER_DIR={}",
        out_path.display()
    );

    for (header, out, whitelist) in headers {
        let out = out_path.join(out);
        if out.exists() {
            continue;
        }
        println!("cargo:rerun-if-changed={}", header);
        println!("cargo:rerun-if-changed={}", out.display());

        let bindings = bindgen::Builder::default()
            .header(header)
            .parse_callbacks(Box::new(bindgen::CargoCallbacks))
            .whitelist_function(whitelist)
            .whitelist_type(whitelist)
            .whitelist_var(whitelist)
            .default_enum_style(bindgen::EnumVariation::ModuleConsts)
            .generate()
            .expect("Unable to generate bindings");
        bindings
            .write_to_file(out)
            .expect("Couldn't write bindings!");
    }

    let icon = image::load(
        BufReader::new(File::open("res/xaskpass.png").expect("icon")),
        image::ImageFormat::Png,
    )
    .expect("loading icon")
    .into_bgra8();

    let icon_raw = icon.as_raw();

    let mut file = File::create(out_path.join("icon.rs")).expect("file");
    writeln!(
        file,
        "const ICONS: &[(u32, u32, &[u8])] = &[({}, {}, &{:?})];",
        icon.width(),
        icon.height(),
        icon_raw
    )
    .unwrap();
}
