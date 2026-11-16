use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo::rerun-if-changed=snowflakes.manifest");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    assert_eq!(env::var("CARGO_CFG_TARGET_ENV").as_deref(), Ok("msvc"));
    assert_eq!(env::var("CARGO_CFG_TARGET_ARCH").as_deref(), Ok("x86_64"));
    let manifest =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("snowflakes.manifest");
    let source = fs::read_to_string(&manifest).expect("client manifest is unavailable");
    let identity = format!(
        "name=\"Snowflakes.Client\" version=\"{}.{}.{}.0\"",
        env::var("CARGO_PKG_VERSION_MAJOR").unwrap(),
        env::var("CARGO_PKG_VERSION_MINOR").unwrap(),
        env::var("CARGO_PKG_VERSION_PATCH").unwrap()
    );
    assert!(
        source.contains(&identity),
        "manifest version must match the package"
    );
    println!("cargo::rustc-link-arg-bin=snowflakes=/MANIFEST:EMBED,ID=1");
    println!(
        "cargo::rustc-link-arg-bin=snowflakes=/MANIFESTINPUT:{}",
        manifest.display()
    );
}
