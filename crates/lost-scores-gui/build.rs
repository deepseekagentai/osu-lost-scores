fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    slint_build::compile_with_config("ui/main.slint", config).unwrap();
    embed_icon();
}

#[cfg(windows)]
fn embed_icon() {
    println!("cargo:rerun-if-changed=assets/icon/lemon.ico");
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("assets/icon/lemon.ico");
    resource.set("ProductName", "osu! Lost Scores Analyzer");
    resource.set("FileDescription", "osu! Lost Scores Analyzer");
    resource.compile().unwrap();
}

#[cfg(not(windows))]
fn embed_icon() {}
