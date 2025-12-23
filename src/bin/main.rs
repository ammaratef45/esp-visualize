#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use embedded_graphics::mono_font::ascii::FONT_4X6;
use embedded_graphics::prelude::{Point, RgbColor};
use embedded_graphics::text::{Alignment, Text};
use embedded_graphics::Drawable;
use esp_hal::clock::CpuClock;
use esp_hal::main;
use esp_hal::time::{Duration, Instant, Rate};
use esp_hal::gpio::Pin;
use esp_hub75::Color;
use esp_hub75::{Hub75, Hub75Pins16, framebuffer::{compute_rows, compute_frame_count, plain::DmaFrameBuffer}};
use embedded_graphics::mono_font::MonoTextStyleBuilder;
use esp_println::println;

const ROWS: usize = 32;
const COLS: usize = 64;
const BITS: u8 = 4;
const NROWS: usize = compute_rows(ROWS);
const FRAME_COUNT: usize = compute_frame_count(BITS);

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
    // generator version: 1.1.0

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);
    let (_, tx_descriptors) = esp_hal::dma_descriptors!(0, FBType::dma_buffer_size_bytes());
    // https://learn.adafruit.com/adafruit-matrixportal-s3/pinouts
    let pins = Hub75Pins16 {
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

    let mut hub75 = Hub75::new_async(
        peripherals.LCD_CAM,
        pins,
        peripherals.DMA_CH0,
        tx_descriptors,
        Rate::from_mhz(20),
    )
    .expect("failed to create Hub75!");

    let mut fb = FBType::new();
    let font = FONT_4X6;
    let text_style = MonoTextStyleBuilder::new()
        .font(&font)
        .text_color(Color::GREEN)
        .background_color(Color::BLACK)
        .build();
    let point = Point::new(0, font.baseline.cast_signed());
    Text::with_alignment("Hello, World!", point, text_style, Alignment::Left)
        .draw(&mut fb)
        .expect("failed to draw text");

    loop {
        let xfer = hub75
            .render(&fb)
            .map_err(|(e, _hub75)| e)
            .expect("failed to start render!");
        let (result, new_hub75) = xfer.wait();
        hub75 = new_hub75;
        result.expect("transfer failed");
    }

    // for inspiration have a look at the examples at https://github.com/esp-rs/esp-hal/tree/esp-hal-v~1.0/examples
}
