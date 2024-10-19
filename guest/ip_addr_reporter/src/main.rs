use api::debian_service_client::DebianServiceClient;
use api::IpAddr;

pub mod api {
    tonic::include_proto!("com.android.virtualization.vmlauncher.proto");
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let gateway_ip_addr = netdev::get_default_gateway()?.ipv4[0];
    let ip_addr = netdev::get_default_interface()?.ipv4[0].addr();
    const PORT: i32 = 12000;

    let server_addr = format!("http://{}:{}", gateway_ip_addr.to_string(), PORT);

    println!("local ip addr: {}", ip_addr.to_string());
    println!("coonect to grpc server {}", server_addr);

    let mut client = DebianServiceClient::connect(server_addr).await.map_err(|e| e.to_string())?;

    let request = tonic::Request::new(IpAddr { addr: ip_addr.to_string() });

    let response = client.report_vm_ip_addr(request).await.map_err(|e| e.to_string())?;
    println!("response from server: {:?}", response);
    Ok(())
}
