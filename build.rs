fn main() {
    let git_version = std::process::Command::new("git")
        .arg("describe")
        .output()
        .unwrap();
    let mut git_version = String::from_utf8(git_version.stdout).unwrap();
    git_version.pop();
    let full_version = git_version.strip_prefix("v").unwrap();

    // commenting this out as it would make the life hard for linters
    //assert!(
    //    full_version.starts_with(&std::env::var("CARGO_PKG_VERSION").unwrap()),
    //    "latest git tag does not match the version set in cargo"
    //);

    println!(
        "cargo:rustc-env=XASKPASS_BUILD_FULL_VERSION={}",
        full_version
    );

    let mut man = std::fs::read_to_string("xaskpass.man.in").unwrap();
    man = man.replace("{VERSION}", full_version);
    std::fs::write("xaskpass.man", man).unwrap();

    let deps = [("xkbcommon", "0.10"), ("xkbcommon-x11", "0.10")];

    if pkg_config::Config::new()
        .atleast_version("1.0")
        .probe("xcb-errors")
        .is_ok()
    {
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
}
