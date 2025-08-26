#![no_std]
extern crate alloc;

use alloc::string::{String, ToString};
use blocking_network_stack::{Socket, Stack};
use core::result::Result;
use embedded_io::Write;
use esp_hal::time;
use esp_println as _;
use esp_wifi::wifi::{self, WifiController, WifiDevice};
use smoltcp::wire::IpAddress;

const SSID: &str = env!("SSID");
const PASSWORD: &str = env!("PASSWORD");
const SERVER_ADDRESS: &str = env!("SERVER_ADDRESS");

pub fn create_interface(device: &mut wifi::WifiDevice) -> smoltcp::iface::Interface {
    smoltcp::iface::Interface::new(
        smoltcp::iface::Config::new(smoltcp::wire::HardwareAddress::Ethernet(
            smoltcp::wire::EthernetAddress::from_bytes(&device.mac_address()),
        )),
        device,
        timestamp(),
    )
}

fn timestamp() -> smoltcp::time::Instant {
    smoltcp::time::Instant::from_micros(
        time::Instant::now().duration_since_epoch().as_micros() as i64
    )
}

pub fn configure_wifi(controller: &mut WifiController<'_>) {
    let client_config = wifi::Configuration::Client(wifi::ClientConfiguration {
        ssid: SSID.try_into().unwrap(),
        password: PASSWORD.try_into().unwrap(),
        ..Default::default()
    });

    let res = controller.set_configuration(&client_config);
    defmt::info!("wifi_set_configuration returned {:?}", res);

    controller.start().unwrap();
    defmt::info!("is wifi started: {:?}", controller.is_started());
}

// pub fn scan_wifi(controller: &mut WifiController<'_>) {
//     defmt::info!("Start Wifi Scan");
//     let res = controller.scan_n(5);
//     if let Ok(res) = res {
//         for ap in res {
//             defmt::info!("{:?}", ap);
//         }
//     }
// }

pub fn connect_wifi(controller: &mut WifiController<'_>) {
    defmt::info!("{:?}", controller.capabilities());
    defmt::info!("wifi_connect {:?}", controller.connect());

    defmt::info!("Wait to get connected");
    loop {
        match controller.is_connected() {
            Ok(true) => break,
            Ok(false) => {}
            Err(err) => panic!("{:?}", err),
        }
    }
    defmt::info!("Connected: {:?}", controller.is_connected());
}

pub fn obtain_ip(stack: &mut Stack<'_, WifiDevice<'_>>) {
    defmt::info!("Wait for IP address");
    loop {
        stack.work();
        if stack.is_iface_up() {
            defmt::info!("IP acquired: {:?}", stack.get_ip_info());
            break;
        }
    }
}

pub fn get_server_addr() -> (IpAddress, u16) {
    let mut addr = SERVER_ADDRESS.split(':');
    let ip = addr.next().unwrap().parse::<IpAddress>().unwrap();
    let port = addr.next().unwrap().parse::<u16>().unwrap();
    (ip, port)
}

pub fn send_request<'a>(
    socket: &'a mut Socket<WifiDevice>,
    ip: IpAddress,
    port: u16,
) -> Result<(), String> {
    defmt::info!("Making request.");
    socket.work();
    socket
        .open(ip, port)
        .map_err(|_| String::from("Cannot connect to server.}"))?;
    if socket.is_open() {
        let request = String::from("GET /stream/status HTTP/1.0\r\nHost: ")
            + ip.to_string().as_str()
            + ":"
            + port.to_string().as_str()
            + "\r\nConnection: close\r\n\r\n";
        socket
            .write(request.as_bytes())
            .map_err(|_| String::from("Write request failed."))?;
        socket
            .flush()
            .map_err(|_| String::from("Cannot send request."))?;
        defmt::info!("Request sent.");
        Ok(())
    } else {
        defmt::error!("Socket is not open");
        Err(String::from("Socket is not open."))
    }
}
