fn main() {
    println!("cargo:rerun-if-changed=../../gfx/vibeEmu.ico");

    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("../../gfx/vibeEmu.ico");
        res.compile().expect("failed to embed Windows icon");
    }
}
