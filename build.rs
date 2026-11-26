fn main() {
    println!("cargo::rustc-check-cfg=cfg(plugins)");
    if std::path::Path::new("src/plugin.rs").exists() {
        println!("cargo::rerun-if-changed=src/plugin.rs");
        println!("cargo::rustc-cfg=plugins");
    }
}
