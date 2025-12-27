use embedded_graphics::{
    mono_font::ascii::FONT_4X6,
    prelude::{Point, RgbColor},
    text::{Alignment, Text},
};
use esp_hal::{
    Async, peripherals,
    gpio::Pin,
    time::Rate
};
use esp_hub75::{
    Color, Hub75, Hub75Pins16,
    framebuffer::{compute_rows, compute_frame_count, plain::DmaFrameBuffer},
};
use embedded_graphics::{
    Drawable,
    mono_font::MonoTextStyleBuilder
};

const EMPTY_LINE: &str = "                                                ";
const ROWS: usize = 32;
const COLS: usize = 64;
const BITS: u8 = 4;
const NROWS: usize = compute_rows(ROWS);
const FRAME_COUNT: usize = compute_frame_count(BITS);
type FBType = DmaFrameBuffer<ROWS, COLS, NROWS, BITS, FRAME_COUNT>;

// https://www.waveshare.com/rgb-matrix-p2.5-64x32.htm
pub struct WaveShare64X32Display<'a> {
    hub75: Hub75<'a, Async>,
    fb: FBType
}

impl<'a> WaveShare64X32Display<'a> {
    pub fn new(
        gpio2: peripherals::GPIO2<'static>,
        gpio14: peripherals::GPIO14<'static>,
        gpio21: peripherals::GPIO21<'static>,
        gpio35: peripherals::GPIO35<'static>,
        gpio36: peripherals::GPIO36<'static>,
        gpio37: peripherals::GPIO37<'static>,
        gpio38: peripherals::GPIO38<'static>,
        gpio39: peripherals::GPIO39<'static>,
        gpio40: peripherals::GPIO40<'static>,
        gpio41: peripherals::GPIO41<'static>,
        gpio42: peripherals::GPIO42<'static>,
        gpio45: peripherals::GPIO45<'static>,
        gpio47: peripherals::GPIO47<'static>,
        gpio48: peripherals::GPIO48<'static>,
        lcd_cam: peripherals::LCD_CAM<'static>,
        dma_ch0: peripherals::DMA_CH0<'static>,
    ) -> Self {
        let (_, tx_descriptors) = esp_hal::dma_descriptors!(0, FBType::dma_buffer_size_bytes());
        // https://learn.adafruit.com/adafruit-matrixportal-s3/pinouts
        let hub75_pins = Hub75Pins16 {
            red1: gpio42.degrade(),
            grn1: gpio41.degrade(),
            blu1: gpio40.degrade(),
            red2: gpio38.degrade(),
            grn2: gpio39.degrade(),
            blu2: gpio37.degrade(),
            addr0: gpio45.degrade(),
            addr1: gpio36.degrade(),
            addr2: gpio48.degrade(),
            addr3: gpio35.degrade(),
            addr4: gpio21.degrade(),
            // MTX_OE
            blank: gpio14.degrade(),
            clock: gpio2.degrade(),
            latch: gpio47.degrade(),
        };
        let hub75: Hub75<'a, Async> = Hub75::new_async(
            lcd_cam,
            hub75_pins,
            dma_ch0,
            tx_descriptors,
            Rate::from_mhz(20),
        ).expect("failed to create Hub75!");
        let fb = FBType::new();
        Self {
            hub75, fb
        }
    }

    fn _draw(mut self, text: &str) -> Self {
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

    pub fn draw(self, text: &str) -> Self {
        let res = self._draw(EMPTY_LINE);
        res._draw(text)
    }
}
