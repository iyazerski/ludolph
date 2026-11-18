//! Embed platform resources in the desktop executable.

use std::path::PathBuf;

const PLIST: &str = "assets/Info.plist";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("macos") => embed_plist(),
        Ok("windows") => {
            println!("cargo:rerun-if-changed=assets/Ludolph.rc");
            println!("cargo:rerun-if-changed=assets/Ludolph.ico");
            embed_resource::compile("assets/Ludolph.rc", embed_resource::NONE)
                .manifest_required()
                .expect("embedding the Windows app icon");
        }
        _ => {}
    }
}

fn embed_plist() {
    // macOS needs the plist for speech and microphone access, even outside an app bundle.
    println!("cargo:rerun-if-changed={PLIST}");
    println!("cargo:rerun-if-env-changed=LUDOLPH_VERSION");
    let template = std::fs::read_to_string(PLIST).expect("reading assets/Info.plist");
    let version = std::env::var("LUDOLPH_VERSION")
        .or_else(|_| std::env::var("CARGO_PKG_VERSION"))
        .expect("cargo sets CARGO_PKG_VERSION");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let plist = out.join("Info.plist");
    std::fs::write(&plist, template.replace("@VERSION@", &version)).expect("writing Info.plist");
    println!(
        "cargo:rustc-link-arg-bins=-Wl,-sectcreate,__TEXT,__info_plist,{}",
        plist.display()
    );
}
