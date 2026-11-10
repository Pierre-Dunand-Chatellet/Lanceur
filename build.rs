// Intègre l'icône dans Lanceur.exe (icône de l'explorateur, de la barre des tâches, des raccourcis).
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico").set("ProductName", "Lanceur").set("FileDescription", "Lanceur");
        res.compile().expect("impossible d'intégrer l'icône");
    }
}
