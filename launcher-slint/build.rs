fn main() {
    println!("cargo:rerun-if-changed=ui/assets/app.ico");
    println!("cargo:rerun-if-changed=ui/assets/app.png");
    println!("cargo:rerun-if-changed=ui/main.slint");

    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("ui/assets/app.ico");
        if let Err(e) = res.compile() {
            eprintln!("Warning: Failed to compile winres icon: {}", e);
        }
    }

    slint_build::compile("ui/main.slint").expect("Slint build failed");
}
