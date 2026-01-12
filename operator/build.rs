fn main() {
    // Only build protos if the proto file exists
    let proto_path = "proto/api.proto";
    if std::path::Path::new(proto_path).exists() {
        tonic_build::configure()
            .build_server(false)
            .compile_protos(&[proto_path], &["proto"])
            .expect("Failed to compile protos");
    } else {
        println!(
            "cargo:warning=Proto file not found at {}, skipping proto generation",
            proto_path
        );
    }
}
