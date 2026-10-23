fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto");
    tonic_build::configure()
        .build_server(false)
        .compile_protos(&["proto/started_service.proto"], &["proto"])?;
    Ok(())
}
