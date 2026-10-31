//! Embeds the icon and version info into the Windows .exe files.

fn main() {
    println!("cargo:rerun-if-changed=assets/ember.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/ember.ico")
        .set("ProductName", "Ember")
        .set("FileDescription", "Ember - screen colour temperature")
        .set("LegalCopyright", "MIT License");
    // A missing resource compiler shouldn't break the build; the exe just has no icon.
    if let Err(e) = res.compile() {
        println!("cargo:warning=could not embed Windows resources: {e}");
    }
}
