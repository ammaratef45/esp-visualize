use embassy_executor::Spawner;
use embassy_net::{DhcpConfig, Runner, StackResources, dns::DnsSocket, tcp::client::{TcpClient, TcpClientState}};
use esp_hal::{peripherals, rng::Rng, timer::timg::TimerGroup};
use esp_println::println;
use esp_radio::wifi::{ClientConfig, ModeConfig, ScanConfig, WifiController, WifiDevice, WifiEvent, WifiStaState};
use reqwless::client::{HttpClient, TlsConfig};


macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

const SSID: &str = env!("WIFI_SSID");
const PASSWORD: &str = env!("WIFI_PASSWORD");

pub struct Wifi {
    stack: embassy_net::Stack<'static>,
    tls_seed: u64,
}

impl Wifi {
    pub fn new(
        wifi_peripheral: peripherals::WIFI<'static>,
        timg_peripheral: peripherals::TIMG0<'static>,
        spawner: &Spawner
    ) -> Self {
        let rng = Rng::new();
        let net_seed = rng.random() as u64 | ((rng.random() as u64) << 32);
        let tls_seed = rng.random() as u64 | ((rng.random() as u64) << 32);
        let (controller, device) = Self::init(wifi_peripheral, timg_peripheral);
        let (stack, runner) = Self::make_stack(device, net_seed);
        spawner.spawn(connection(controller)).ok();
        spawner.spawn(net_task(runner)).ok();
        Self{
            stack, tls_seed
        }
    }

    fn init(
        wifi_peripheral: peripherals::WIFI<'static>,
        timg_peripheral: peripherals::TIMG0<'static>,
    ) -> (WifiController<'static>, WifiDevice<'static>) {
        let timg0 = TimerGroup::new(timg_peripheral);
        esp_rtos::start(timg0.timer0);
        let radio = &*mk_static!(
            esp_radio::Controller<'static>,
            esp_radio::init().expect("Failed to initialize Wi-Fi/BLE controller")
        );
        let (mut wifi_controller, interfaces) =
            esp_radio::wifi::new(&radio, wifi_peripheral, Default::default())
                .expect("Failed to init Wi-Fi");
        let device = interfaces.sta;
        wifi_controller
            .set_power_saving(esp_radio::wifi::PowerSaveMode::None)
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

    pub async fn wait_for_connection(&self) {
        println!("Waiting for link to be up");
        loop {
            if self.stack.is_link_up() {
                break;
            }
            embassy_time::Timer::after(embassy_time::Duration::from_millis(500)).await;
        }

        println!("Waiting to get IP address...");
        loop {
            if let Some(config) = self.stack.config_v4() {
                println!("Got IP: {}", config.address);
                break;
            }
            embassy_time::Timer::after(embassy_time::Duration::from_millis(500)).await;
        }
    }

    pub async fn get(&self, url: &str) {
        let mut rx_buffer = [0; 4096];
        let mut tx_buffer = [0; 4096];
        let dns = DnsSocket::new(self.stack);
        let tcp_state = TcpClientState::<1, 4096, 4096>::new();
        let tcp: TcpClient<'_, 1, 4096, 4096> = TcpClient::new(self.stack, &tcp_state);

        let tls = TlsConfig::new(
            self.tls_seed,
            &mut rx_buffer,
            &mut tx_buffer,
            reqwless::client::TlsVerify::None,
        );

        let mut client = HttpClient::new_with_tls(&tcp, &dns, tls);
        let mut buffer = [0u8; 4096];
        let mut http_req = client
            .request(
                reqwless::request::Method::GET,
                url,
            )
            .await
            .unwrap();
        let response = http_req.send(&mut buffer).await.unwrap();

        println!("Got response");
        let res = response.body().read_to_end().await.unwrap();

        let content = core::str::from_utf8(res).unwrap();
        println!("{}", content);
    }

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
