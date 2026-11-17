fn main() {
    if let Err(e) = pkg_config::Config::new().atleast_version("2.0").probe("mpv") {
        panic!("libmpv not found via pkg-config: {e}");
    }
}
