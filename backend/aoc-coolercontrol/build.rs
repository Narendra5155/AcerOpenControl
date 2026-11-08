// Compile CoolerControl's protobuf definitions with protox (pure Rust), so no
// system `protoc` is needed to build. The client is only used by examples/e2e.rs.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let files =
        ["proto/coolercontrol/models/v1/device.proto", "proto/coolercontrol/device_service/v1/device_service.proto"];
    let fds = protox::compile(files, ["proto"])?;
    tonic_prost_build::configure().build_server(true).build_client(true).compile_fds(fds)?;
    println!("cargo:rerun-if-changed=proto");
    Ok(())
}
