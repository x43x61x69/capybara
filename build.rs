fn main() {
    // Shared modules also compile in the standalone no_std H717 core.
    println!("cargo:rustc-check-cfg=cfg(feature, values(\"embedded\"))");
    println!("cargo:rerun-if-changed=assets/capybara.ico");
    #[cfg(windows)]
    {
        let mut ressource = winresource::WindowsResource::new();
        ressource.set_icon("assets/capybara.ico");
        if let Err(e) = ressource.compile() {
            panic!("icone Windows impossible a integrer : {e}");
        }
    }
}
