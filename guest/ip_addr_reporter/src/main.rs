use api::debian_service_client::DebianServiceClient;
use api::IpAddr;

use clap::Parser;
pub mod api {
    tonic::include_proto!("com.android.virtualization.terminal.proto");
}

#[derive(Parser)]
/// Flags for running command
pub struct Args {
    /// grpc port number
    #[arg(long)]
    #[arg(alias = "grpc_port")]
    grpc_port: String,
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let args = Args::parse();
    let gateway_ip_addr = netdev::get_default_gateway()?.ipv4[0];
    let ip_addr = netdev::get_default_interface()?.ipv4[0].addr();

    let server_addr = format!("http://{}:{}", gateway_ip_addr.to_string(), args.grpc_port);

    println!("local ip addr: {}", ip_addr.to_string());
    println!("coonect to grpc server {}", server_addr);

    let mut client = DebianServiceClient::connect(server_addr).await.map_err(|e| e.to_string())?;

    let request = tonic::Request::new(IpAddr { addr: ip_addr.to_string() });

    let response = client.report_vm_ip_addr(request).await.map_err(|e| e.to_string())?;
    println!("response from server: {:?}", response);
    Ok(())
}
