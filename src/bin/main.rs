#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

extern crate alloc;

use blocking_network_stack::Stack;
use esp_alloc::{self as _};
use embedded_graphics::mono_font::ascii::FONT_4X6;
use embedded_graphics::prelude::{Point, RgbColor};
use embedded_graphics::text::{Alignment, Text};
use embedded_graphics::Drawable;
use esp_hal::clock::CpuClock;
use esp_hal::delay::Delay;
use esp_hal::dma::DmaDescriptor;
use esp_hal::rng::Rng;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::{Async, main};
use esp_hal::peripherals::{Peripherals, TIMG0, WIFI};
use esp_hal::time::{Duration, Instant, Rate};
use esp_hal::gpio::Pin;
use esp_hub75::Color;
use esp_hub75::{Hub75, Hub75Pins16, framebuffer::{compute_rows, compute_frame_count, plain::DmaFrameBuffer}};
use embedded_graphics::mono_font::MonoTextStyleBuilder;
use esp_radio::wifi::{self, ClientConfig, ModeConfig, ScanConfig, WifiController, WifiDevice};
use smoltcp::iface::{SocketSet, SocketStorage};
use esp_println::println;
use smoltcp::wire::IpAddress;
use embedded_io::{Read, Write};

const ROWS: usize = 32;
const COLS: usize = 64;
const BITS: u8 = 4;
const NROWS: usize = compute_rows(ROWS);
const FRAME_COUNT: usize = compute_frame_count(BITS);
const SSID: &str = env!("WIFI_SSID");
const PASSWORD: &str = env!("WIFI_PASSWORD");

type FBType = DmaFrameBuffer<ROWS, COLS, NROWS, BITS, FRAME_COUNT>;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[main]
fn main() -> ! {
    let peripherals = init_hardware();
    let (_, tx_descriptors) = esp_hal::dma_descriptors!(0, FBType::dma_buffer_size_bytes());

    let (hub75, timg0, wifi_peripheral) = peripherals_extraction(peripherals, tx_descriptors);

    let mut matrix_display = WaveShare64X32Display::new(hub75);

    esp_rtos::start(timg0.timer0);
    let radio = esp_radio::init().expect("Failed to init radio");
    let (mut controller, device) = init_wifi(wifi_peripheral, &radio);
    scan_wifi(&mut controller);
    connect_wifi(&mut controller);
    let mut socket_set_entries: [SocketStorage; 4] = Default::default();
    let mut stack = make_stack(device, &mut socket_set_entries);
    obtain_ip(&mut stack);
    let mut rx_buffer = [0u8; 1536];
    let mut tx_buffer = [0u8; 1536];
    let socket = stack.get_socket(&mut rx_buffer, &mut tx_buffer);
    http_request(socket);

    loop {
        matrix_display = matrix_display.draw("Hi Again!");
    }
}

fn init_hardware() -> Peripherals {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);
    esp_alloc::heap_allocator!(size: 72 * 1024);
    peripherals
}

fn peripherals_extraction<'a>(peripherals: Peripherals, tx_descriptors: &'static mut [DmaDescriptor]) -> 
(Hub75<'a, Async>, TimerGroup<'a, TIMG0<'a>>, WIFI<'static>) {
    // https://learn.adafruit.com/adafruit-matrixportal-s3/pinouts
    let hub75_pins = Hub75Pins16 {
        red1: peripherals.GPIO42.degrade(),
        grn1: peripherals.GPIO41.degrade(),
        blu1: peripherals.GPIO40.degrade(),
        red2: peripherals.GPIO38.degrade(),
        grn2: peripherals.GPIO39.degrade(),
        blu2: peripherals.GPIO37.degrade(),
        addr0: peripherals.GPIO45.degrade(),
        addr1: peripherals.GPIO36.degrade(),
        addr2: peripherals.GPIO48.degrade(),
        addr3: peripherals.GPIO35.degrade(),
        addr4: peripherals.GPIO21.degrade(),
        // MTX_OE
        blank: peripherals.GPIO14.degrade(),
        clock: peripherals.GPIO2.degrade(),
        latch: peripherals.GPIO47.degrade(),
    };
    let hub75 = Hub75::new_async(
        peripherals.LCD_CAM,
        hub75_pins,
        peripherals.DMA_CH0,
        tx_descriptors,
        Rate::from_mhz(20),
    ).expect("failed to create Hub75!");

    // esp-radio stuff
    // esp-rtos (otherwise esp-radio will yell at you "`esp-radio` has no scheduler enabled.")
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let wifi_peripheral = peripherals.WIFI;

    // Returns
    (hub75, timg0, wifi_peripheral)
}

// https://www.waveshare.com/rgb-matrix-p2.5-64x32.htm
struct WaveShare64X32Display<'a> {
    hub75: Hub75<'a, Async>,
    fb: FBType
}

impl<'a> WaveShare64X32Display<'a> {
    pub fn new(hub75: Hub75<'a, Async>) -> Self {
        let fb = FBType::new();
        Self {
            hub75, fb
        }
    }

    fn draw(mut self, text: &str) -> Self {
        let font = FONT_4X6;
        let text_style = MonoTextStyleBuilder::new()
            .font(&font)
            .text_color(Color::GREEN)
            .background_color(Color::BLACK)
            .build();
        let point = Point::new(0, font.baseline.cast_signed());
        Text::with_alignment(text, point, text_style, Alignment::Left)
            .draw(&mut self.fb)
            .expect("failed to draw text");
        let xfer = self.hub75
            .render(&self.fb)
            .map_err(|(e, _hub75)| e)
            .expect("failed to start render!");
        let (result, new_hub75) = xfer.wait();
        self.hub75 = new_hub75;
        result.expect("transfer failed");
        self
    }
}

fn init_wifi<'a>(wifi_peripheral: WIFI<'static>, radio: &'a esp_radio::Controller) -> (WifiController<'a>, WifiDevice<'a>) {
    let (mut wifi_controller, interfaces) =
        wifi::new(&radio, wifi_peripheral, Default::default())
            .expect("Failed to init Wi-Fi");
    let device = interfaces.sta;
    wifi_controller
        .set_power_saving(wifi::PowerSaveMode::None)
        .unwrap();
    let client_cfg = ModeConfig::Client(
        ClientConfig::default()
            .with_ssid(SSID.into())
            .with_password(PASSWORD.into()),
    );
    wifi_controller.set_config(&client_cfg).unwrap();
    wifi_controller.start().unwrap();
    (wifi_controller, device)
}

fn scan_wifi(controller: &mut WifiController<'_>) {
    println!("Start Wifi Scan");
    let scan_config = ScanConfig::default().with_max(10);
    let res = controller.scan_with_config(scan_config).unwrap();
    for ap in res {
        println!("{:?}", ap);
    }
}

fn connect_wifi(controller: &mut WifiController<'_>) {
    println!("{:?}", controller.capabilities());
    println!("wifi_connect {:?}", controller.connect());

    println!("Wait to get connected");
    loop {
        match controller.is_connected() {
            Ok(true) => break,
            Ok(false) => {}
            Err(err) => panic!("{:?}", err),
        }
    }
    println!("Connected: {:?}", controller.is_connected());
}

pub fn create_interface(device: &mut esp_radio::wifi::WifiDevice) -> smoltcp::iface::Interface {
    smoltcp::iface::Interface::new(
        smoltcp::iface::Config::new(smoltcp::wire::HardwareAddress::Ethernet(
            smoltcp::wire::EthernetAddress::from_bytes(&device.mac_address()),
        )),
        device,
        timestamp(),
    )
}

// some smoltcp boilerplate
fn timestamp() -> smoltcp::time::Instant {
    smoltcp::time::Instant::from_micros(
        esp_hal::time::Instant::now()
            .duration_since_epoch()
            .as_micros() as i64,
    )
}

fn obtain_ip(stack: &mut Stack<esp_radio::wifi::WifiDevice<'_>>) {
    println!("Wait for IP address");
    loop {
        stack.work();
        if stack.is_iface_up() {
            println!("IP acquired: {:?}", stack.get_ip_info());
            break;
        }
    }
}

fn make_stack<'a>(
    mut device: WifiDevice<'a>,
    socket_set_entries: &'a mut [SocketStorage<'a>; 4]
) -> Stack<'a, WifiDevice<'a>> {
    let iface = create_interface(&mut device);
    let mut socket_set = SocketSet::new( &mut socket_set_entries[..]);
    let dhcp_socket = smoltcp::socket::dhcpv4::Socket::new();
    socket_set.add(dhcp_socket);
    let now_fn = || Instant::now().duration_since_epoch().as_millis();

    Stack::new(
        iface,
        device,
        socket_set,
        now_fn,
        Rng::new().random()
    )
}

fn http_request(mut socket: blocking_network_stack::Socket<'_, '_, esp_radio::wifi::WifiDevice<'_>>) {
    println!("Starting HTTP client loop");
    let delay = Delay::new();
    println!("Making HTTP request");
    socket.work();
    let remote_addr = IpAddress::v4(142, 250, 185, 115);
    socket.open(remote_addr, 80).unwrap();
    socket.write(b"GET / HTTP/1.0\r\nHost: www.mobile-j.de\r\n\r\n").unwrap();
    socket.flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut buffer = [0u8; 512];
    while let Ok(len) = socket.read(&mut buffer) {
        let Ok(text) = core::str::from_utf8(&buffer[..len]) else {
            panic!("Invalid UTF-8 sequence encountered");
        };
        println!("{}", text);
        if Instant::now() > deadline {
            println!("Timeout");
            break;
        }
    }
    socket.disconnect();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        socket.work();
    }
    delay.delay_millis(1000);
}
