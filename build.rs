fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    println!("cargo:rerun-if-changed=assets/icone-assinador-livre.ico");

    #[cfg(windows)]
    if target_os == "windows" {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/icone-assinador-livre.ico");
        if let Err(err) = res.compile() {
            panic!("Falha ao compilar recursos do Windows: {err}");
        }
    }
}
