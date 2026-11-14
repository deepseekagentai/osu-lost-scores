fn main() {
    let debug = std::env::var("PROFILE").is_ok_and(|profile| profile == "debug");
    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent-dark".into())
        .with_debug_info(debug);
    slint_build::compile_with_config("ui/main.slint", config).unwrap();
    if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
        embed_icon();
    }
}

fn embed_icon() {
    println!("cargo:rerun-if-changed=assets/icon/lemon.ico");
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("assets/icon/lemon.ico");
    resource.set("ProductName", "osu! Lost Scores Analyzer");
    resource.set("FileDescription", "osu! Lost Scores Analyzer");
    resource.compile().unwrap();
}
