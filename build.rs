fn main() {
    #[cfg(target_os = "windows")]
    {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/joocode.ico");
        resource
            .compile()
            .expect("failed to embed the Joocode application icon");
    }
}
