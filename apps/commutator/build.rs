fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        // Read at run time: `env!` would bake in the folder the build script
        // was compiled in, and a moved checkout then links a missing file.
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo");
        let plist = std::path::Path::new(&manifest_dir).join("../../dist/macos/Info-unbundled.plist");
        println!("cargo:rerun-if-changed={}", plist.display());
        // `cargo run` has no .app Info.plist. NSBundle also reads privacy
        // declarations from this Mach-O section, so source builds can request
        // microphone access without requiring a separate packaging step.
        println!(
            "cargo:rustc-link-arg-bin=commutator=-Wl,-sectcreate,__TEXT,__info_plist,{}",
            plist.display()
        );
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // Only here: a missing file in rerun-if-changed reruns the script on
        // every build, and dist/windows isn't in every checkout.
        println!("cargo:rerun-if-changed=../../dist/windows/commutator.rc");
        println!("cargo:rerun-if-changed=../../dist/windows/commutator.ico");
        embed_resource::compile_for(
            "../../dist/windows/commutator.rc",
            &["commutator"],
            embed_resource::NONE,
        )
        .manifest_required()
        .expect("Windows app icon resource compilation failed");
    }
}
