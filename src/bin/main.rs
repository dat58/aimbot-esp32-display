#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

use aimbot_esp32_display::*;
use blocking_network_stack::Stack;
use embedded_graphics::{
    mono_font::{ascii, MonoTextStyle},
    pixelcolor::BinaryColor,
    prelude::*,
    text::{Alignment, Text},
};
use embedded_hal::delay::DelayNs;
use embedded_io::Read;
use esp_hal::{
    clock::CpuClock, delay::Delay, i2c::master::I2c, main, time, timer::timg::TimerGroup,
};
use esp_println as _;
use esp_wifi::wifi;
use smoltcp::iface::{SocketSet, SocketStorage};
use smoltcp::socket::dhcpv4::RetryConfig;
use smoltcp::time::Duration as SmolDuration;
use smoltcp::wire::DhcpOption;
use ssd1306::{prelude::*, I2CDisplayInterface, Ssd1306};

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

extern crate alloc;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

#[main]
fn main() -> ! {
    // generator version: 0.5.0

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(size: 74 * 1024);

    let i2c = I2c::new(peripherals.I2C0, Default::default())
        .unwrap()
        .with_sda(peripherals.GPIO21)
        .with_scl(peripherals.GPIO22);

    let interface = I2CDisplayInterface::new(i2c);
    let mut display = Ssd1306::new(interface, DisplaySize128x64, DisplayRotation::Rotate0)
        .into_buffered_graphics_mode();

    let mut delay = Delay::new();
    display.init().unwrap();
    display.clear(BinaryColor::Off).unwrap();
    display.flush().unwrap();

    let text_style = MonoTextStyle::new(&ascii::FONT_7X13_BOLD, BinaryColor::On);

    Text::with_alignment(
        "Hacking...",
        Point::new(64, 32),
        MonoTextStyle::new(&ascii::FONT_9X18_BOLD, BinaryColor::On),
        Alignment::Center,
    )
    .draw(&mut display)
    .unwrap();
    display.flush().unwrap();

    let (ip, port) = get_server_addr();

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let mut rng = esp_hal::rng::Rng::new(peripherals.RNG);

    let esp_wifi_ctrl = esp_wifi::init(timg0.timer0, rng.clone()).unwrap();
    let (mut controller, interfaces) = wifi::new(&esp_wifi_ctrl, peripherals.WIFI).unwrap();
    let mut device = interfaces.sta;
    let mut socket_set_entries: [SocketStorage; 3] = Default::default();
    let mut socket_set = SocketSet::new(&mut socket_set_entries[..]);

    let mut dhcp_socket = smoltcp::socket::dhcpv4::Socket::new();
    // we can set a hostname here (or add other DHCP options)
    dhcp_socket.set_outgoing_options(&[DhcpOption {
        kind: 12,
        data: b"implRust",
    }]);

    let mut retry_config = RetryConfig::default();
    retry_config.discover_timeout = SmolDuration::from_secs(2);
    retry_config.initial_request_timeout = SmolDuration::from_secs(2);
    retry_config.request_retries = 3;
    retry_config.min_renew_timeout = SmolDuration::from_secs(1);
    retry_config.max_renew_timeout = SmolDuration::from_secs(5);
    dhcp_socket.set_retry_config(retry_config);

    socket_set.add(dhcp_socket);

    let now = || time::Instant::now().duration_since_epoch().as_millis();
    let mut stack = Stack::new(
        create_interface(&mut device),
        device,
        socket_set,
        now,
        rng.random(),
    );

    configure_wifi(&mut controller);
    connect_wifi(&mut controller);
    obtain_ip(&mut stack);

    let mut rx_buffer = [0u8; 1536];
    let mut tx_buffer = [0u8; 1536];
    let mut socket = stack.get_socket(&mut rx_buffer, &mut tx_buffer);

    loop {
        // clear screen
        display.clear(BinaryColor::Off).unwrap();

        // send request
        let status = send_request(&mut socket, ip, port);

        match status {
            Ok(_) => {}
            Err(err) => {
                defmt::error!("{}", err.as_str());
                Text::with_alignment(
                    "! Game Over !",
                    Point::new(64, 32),
                    MonoTextStyle::new(&ascii::FONT_9X18_BOLD, BinaryColor::On),
                    Alignment::Center,
                )
                .draw(&mut display)
                .unwrap();
                display.flush().unwrap();
                delay.delay(time::Duration::from_hours(1));
            }
        }

        let mut buffer = [0u8; 512];
        if let Ok(len) = socket.read(&mut buffer) {
            let Ok(text) = core::str::from_utf8(&buffer[..len]) else {
                panic!("Invalid UTF-8 sequence encountered");
            };
            defmt::info!("HTTP response: {}", text);
            let text = text.split("\r\n\r\n").collect::<alloc::vec::Vec<&str>>();
            let mut body = text.get(1).unwrap().split(",");
            let aim_signal = body.next().unwrap();
            let aim_mode = body.next().unwrap();
            let text = alloc::format!("Hacking: {aim_signal}\nAimMode: {aim_mode}");

            Text::with_alignment(
                text.as_str(),
                Point::new(10, 28),
                text_style,
                Alignment::Left,
            )
            .draw(&mut display)
            .unwrap();
            display.flush().unwrap();
        }

        socket.disconnect();
        delay.delay_ms(1_000);
    }
}
