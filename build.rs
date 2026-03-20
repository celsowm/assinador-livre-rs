fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    slint_build::compile("ui/cert_dialog.slint").expect("Falha ao compilar UI Slint");
    println!("cargo:rerun-if-changed=assets/icone-assinador-livre.ico");
    println!("cargo:rerun-if-changed=ui/cert_dialog.slint");
    println!("cargo:rerun-if-changed=third_party/pdfium/windows-x64/pdfium.dll");
    println!("cargo:rerun-if-changed=third_party/pdfium/linux-x64/libpdfium.so");
    println!("cargo:rerun-if-changed=third_party/pdfium/macos-x64/libpdfium.dylib");

    #[cfg(windows)]
    if target_os == "windows" {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/icone-assinador-livre.ico");
        if let Err(err) = res.compile() {
            panic!("Falha ao compilar recursos do Windows: {err}");
        }
    }
}
