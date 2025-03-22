fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_file = "../../libs/debian_service/proto/DebianService.proto";

    tonic_build::compile_protos(proto_file).unwrap();

    Ok(())
}
