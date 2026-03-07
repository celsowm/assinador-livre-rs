fn main() {
    println!("cargo:rerun-if-changed=assets/icone-assinador-livre.ico");

    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/icone-assinador-livre.ico");
        if let Err(err) = res.compile() {
            panic!("Falha ao compilar recursos do Windows: {err}");
        }
    }
}
