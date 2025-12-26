#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]
#![feature(type_alias_impl_trait)]

extern crate alloc;

use esp_alloc::{self as _};
use embedded_graphics::mono_font::ascii::FONT_4X6;
use embedded_graphics::prelude::{Point, RgbColor};
use embedded_graphics::text::{Alignment, Text};
use embedded_graphics::Drawable;
use esp_hal::clock::CpuClock;
use esp_hal::dma::DmaDescriptor;
use esp_hal::rng::Rng;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::Async;
use esp_hal::peripherals::{Peripherals, TIMG0, WIFI};
use esp_hal::time::Rate;
use esp_hal::gpio::Pin;
use esp_hub75::Color;
use esp_hub75::{Hub75, Hub75Pins16, framebuffer::{compute_rows, compute_frame_count, plain::DmaFrameBuffer}};
use embedded_graphics::mono_font::MonoTextStyleBuilder;
use esp_radio::wifi::{self, ClientConfig, ModeConfig, ScanConfig, WifiController, WifiDevice, WifiEvent, WifiStaState};
use esp_println::println;
use embassy_net::{
    DhcpConfig, Runner, StackResources,
    dns::DnsSocket,
    tcp::client::{TcpClient, TcpClientState},
};
use embassy_executor::Spawner;
use reqwless::client::{HttpClient, TlsConfig};

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

macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}


#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    let peripherals = init_hardware();
    let (_, tx_descriptors) = esp_hal::dma_descriptors!(0, FBType::dma_buffer_size_bytes());

    let (hub75, timg0, wifi_peripheral) = peripherals_extraction(peripherals, tx_descriptors);
    let mut matrix_display = WaveShare64X32Display::new(hub75);

    esp_rtos::start(timg0.timer0);
    let radio = &*mk_static!(
        esp_radio::Controller<'static>,
        esp_radio::init().expect("Failed to initialize Wi-Fi/BLE controller")
    );
    let rng = Rng::new();
    let net_seed = rng.random() as u64 | ((rng.random() as u64) << 32);
    let tls_seed = rng.random() as u64 | ((rng.random() as u64) << 32);
    let (controller, device) = init_wifi(wifi_peripheral, &radio);
    let (stack, runner) = make_stack(device, net_seed);

    spawner.spawn(connection(controller)).ok();
    spawner.spawn(net_task(runner)).ok();

    wait_for_connection(stack).await;
    access_website(stack, tls_seed).await;

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

#[embassy_executor::task]
async fn connection(mut controller: WifiController<'static>) {
    println!("start connection task");
    println!("Device capabilities: {:?}", controller.capabilities());
    loop {
        match esp_radio::wifi::sta_state() {
            WifiStaState::Connected => {
                // wait until we're no longer connected
                controller.wait_for_event(WifiEvent::StaDisconnected).await;
                embassy_time::Timer::after(embassy_time::Duration::from_millis(5000)).await
            }
            _ => {}
        }
        if !matches!(controller.is_started(), Ok(true)) {
            let client_config = ModeConfig::Client(
                ClientConfig::default()
                    .with_ssid(SSID.into())
                    .with_password(PASSWORD.into()),
            );
            controller.set_config(&client_config).unwrap();
            println!("Starting wifi");
            controller.start_async().await.unwrap();
            println!("Wifi started!");

            println!("Scan");
            let scan_config = ScanConfig::default().with_max(10);
            let result = controller
                .scan_with_config_async(scan_config)
                .await
                .unwrap();
            for ap in result {
                println!("{:?}", ap);
            }
        }
        println!("About to connect...");

        match controller.connect_async().await {
            Ok(_) => println!("Wifi connected!"),
            Err(e) => {
                println!("Failed to connect to wifi: {:?}", e);
                embassy_time::Timer::after(embassy_time::Duration::from_millis(5000)).await
            }
        }
    }
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, WifiDevice<'static>>) {
    runner.run().await
}

async fn wait_for_connection(stack: embassy_net::Stack<'_>) {
    println!("Waiting for link to be up");
    loop {
        if stack.is_link_up() {
            break;
        }
        embassy_time::Timer::after(embassy_time::Duration::from_millis(500)).await;
    }

    println!("Waiting to get IP address...");
    loop {
        if let Some(config) = stack.config_v4() {
            println!("Got IP: {}", config.address);
            break;
        }
        embassy_time::Timer::after(embassy_time::Duration::from_millis(500)).await;
    }
}

fn make_stack<'a>(
    device: WifiDevice<'a>, net_seed: u64
) -> (embassy_net::Stack<'a>, Runner<'a, WifiDevice<'a>>) {
    let dhcp_config = DhcpConfig::default();
    let config = embassy_net::Config::dhcpv4(dhcp_config);

    embassy_net::new(
        device,
        config,
        mk_static!(StackResources<3>, StackResources::<3>::new()),
        net_seed
    )
}

async fn access_website(stack: embassy_net::Stack<'_>, tls_seed: u64) {
    let mut rx_buffer = [0; 4096];
    let mut tx_buffer = [0; 4096];
    let dns = DnsSocket::new(stack);
    let tcp_state = TcpClientState::<1, 4096, 4096>::new();
    let tcp = TcpClient::new(stack, &tcp_state);

    let tls = TlsConfig::new(
        tls_seed,
        &mut rx_buffer,
        &mut tx_buffer,
        reqwless::client::TlsVerify::None,
    );

    let mut client = HttpClient::new_with_tls(&tcp, &dns, tls);
    let mut buffer = [0u8; 4096];
    let mut http_req = client
        .request(
            reqwless::request::Method::GET,
            "https://jsonplaceholder.typicode.com/posts/1",
        )
        .await
        .unwrap();
    let response = http_req.send(&mut buffer).await.unwrap();

    println!("Got response");
    let res = response.body().read_to_end().await.unwrap();

    let content = core::str::from_utf8(res).unwrap();
    println!("{}", content);
}
