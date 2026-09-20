fn main() {
    println!("cargo:rerun-if-changed=../../assets/app-icons/lexift.ico");
    println!("cargo:rerun-if-changed=../../assets/windows/app.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon("../../assets/app-icons/lexift.ico")
        .set_manifest_file("../../assets/windows/app.manifest");
    resource
        .compile()
        .expect("failed to compile Lexift Windows resources");
}
