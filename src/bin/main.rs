#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

use alloc::string::ToString;
use aimbot_esp32_display::*;
use blocking_network_stack::Stack;
use embedded_graphics::{
    mono_font::{ascii, MonoTextStyle},
    pixelcolor::Rgb565,
    prelude::*,
    text::{Alignment, Text},
};
use embedded_hal::delay::DelayNs;
use embedded_hal_bus::spi::ExclusiveDevice;
use embedded_io::Read;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::spi::master::Config as SpiConfig;
use esp_hal::spi::master::Spi;
use esp_hal::spi::Mode as SpiMode;
use esp_hal::time::Rate;
use esp_hal::{clock::CpuClock, delay::Delay, main, time, timer::timg::TimerGroup};
use esp_println as _;
use esp_wifi::wifi;
use smoltcp::iface::{SocketSet, SocketStorage};
use smoltcp::socket::dhcpv4::RetryConfig;
use smoltcp::time::Duration as SmolDuration;
use smoltcp::wire::DhcpOption;
use st7735_lcd::ST7735;

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

    let spi = Spi::new(
        peripherals.SPI2,
        SpiConfig::default()
            .with_frequency(Rate::from_mhz(4))
            .with_mode(SpiMode::_0),
    )
    .unwrap()
    .with_sck(peripherals.GPIO18)
    .with_mosi(peripherals.GPIO23);
    let cs = Output::new(peripherals.GPIO15, Level::Low, OutputConfig::default());
    let dc = Output::new(peripherals.GPIO2, Level::Low, OutputConfig::default());
    let reset = Output::new(peripherals.GPIO4, Level::Low, OutputConfig::default());
    let spi_dev = ExclusiveDevice::new_no_delay(spi, cs).unwrap();
    let mut display = ST7735::new(spi_dev, dc, reset, true, false, 160, 128);

    let mut delay = Delay::new();
    display.init(&mut delay).unwrap();
    // display.set_orientation(&Orientation::Landscape).unwrap();
    display.clear(Rgb565::RED).unwrap();

    Text::with_alignment(
        "Hacking...",
        Point::new(64, 80),
        MonoTextStyle::new(&ascii::FONT_9X18_BOLD, Rgb565::WHITE),
        Alignment::Center,
    )
    .draw(&mut display)
    .unwrap();

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
    let text_style = MonoTextStyle::new(&ascii::FONT_9X18_BOLD, Rgb565::BLUE);

    let mut old_aim_signal = alloc::string::String::new();
    let mut old_aim_mode = alloc::string::String::new();
    loop {
        // send request
        let status = send_request(&mut socket, ip, port);

        match status {
            Ok(_) => {}
            Err(err) => {
                defmt::error!("{}", err.as_str());
                Text::with_alignment(
                    "! Game Over !",
                    Point::new(64, 80),
                    MonoTextStyle::new(&ascii::FONT_9X18_BOLD, Rgb565::RED),
                    Alignment::Center,
                )
                .draw(&mut display)
                .unwrap();
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
            if old_aim_signal != aim_signal || old_aim_mode != aim_mode {
                // clear screen
                display.clear(Rgb565::BLACK).unwrap();
                Text::with_alignment(
                    text.as_str(),
                    Point::new(10, 28),
                    text_style,
                    Alignment::Left,
                )
                .draw(&mut display)
                .unwrap();
                old_aim_signal = aim_signal.to_string();
                old_aim_mode = aim_mode.to_string();
            }
        }

        socket.disconnect();
        delay.delay_ms(1_000);
    }
}
