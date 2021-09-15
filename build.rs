use std::fs::File;
use std::io::BufReader;
use std::io::Write as _;
use std::path::{Path, PathBuf};

fn get_git_version() -> Result<String, Box<dyn std::error::Error>> {
    if Path::new(".git/HEAD").exists() {
        println!("cargo:rerun-if-changed=.git/HEAD");
    }
    let git_version = std::process::Command::new("git").arg("describe").output()?;
    let mut git_version = String::from_utf8(git_version.stdout)?;
    git_version.pop();
    let full_version = git_version.strip_prefix('v').ok_or("error")?.to_owned();
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

    let deps = [("xkbcommon", "0.10"), ("xkbcommon-x11", "0.10")];

    for (dep, version) in &deps {
        if let Err(s) = pkg_config::Config::new()
            .atleast_version(version)
            .probe(dep)
        {
            eprintln!("{}", s);
            std::process::exit(1);
        }
    }

    let headers = vec![
        ("src/keyboard/ffi.h", "xkbcommon.rs", "xkb_.*|XKB_.*"),
        ("src/keyboard/ffi_names.h", "xkbcommon-names.rs", ".*"),
        ("src/keyboard/ffi_keysyms.h", "xkbcommon-keysyms.rs", ".*"),
    ];

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
            .allowlist_function(whitelist)
            .allowlist_type(whitelist)
            .allowlist_var(whitelist)
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
