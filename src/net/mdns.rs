use esp_idf_svc::mdns::EspMdns;
use esp_idf_svc::sys::EspError;

const SERVICE: &str = "_beamer";
const PROTO: &str = "_tcp";
const PORT: u16 = 80;

pub fn advertise(hostname: &str) -> Result<EspMdns, EspError> {
    let mut mdns = EspMdns::take()?;
    mdns.set_hostname(hostname)?;
    mdns.set_instance_name(hostname)?;
    mdns.add_service(Some(hostname), SERVICE, PROTO, PORT, &[])?;

    log::info!("mdns: {hostname}.local advertising {SERVICE}{PROTO} on {PORT}");
    Ok(mdns)
}
