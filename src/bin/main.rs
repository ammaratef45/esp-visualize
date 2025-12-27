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
use esp_hal::clock::CpuClock;
use esp_hal::peripherals::Peripherals;
use embassy_executor::Spawner;
use esp_visualize::wifi::Wifi;
use esp_visualize::display::WaveShare64X32Display;

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


#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    let peripherals = init_hardware();
    let mut matrix_display = WaveShare64X32Display::new(
        peripherals.GPIO2, peripherals.GPIO14, peripherals.GPIO21,
        peripherals.GPIO35, peripherals.GPIO36, peripherals.GPIO37,
        peripherals.GPIO38, peripherals.GPIO39, peripherals.GPIO40,
        peripherals.GPIO41, peripherals.GPIO42, peripherals.GPIO45,
        peripherals.GPIO47, peripherals.GPIO48, peripherals.LCD_CAM,
        peripherals.DMA_CH0
    );
    let wifi = Wifi::new(peripherals.WIFI, peripherals.TIMG0, &spawner);
    wifi.wait_for_connection().await;
    wifi.get("https://jsonplaceholder.typicode.com/posts/1").await;

    loop {
        matrix_display = matrix_display.draw("Connected!");
    }
}

fn init_hardware() -> Peripherals {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);
    esp_alloc::heap_allocator!(size: 72 * 1024);
    peripherals
}
