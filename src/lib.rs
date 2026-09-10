use std::fmt;
use std::fs;
use std::io;
use std::net::{IpAddr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

pub const SYSFS_ROOT: &str = "/sys/class/leds";
pub const SYSFS_NET_ROOT: &str = "/sys/class/net";
pub const UCI_BIN: &str = "/sbin/uci";

const PING_BIN: &str = "/bin/ping";
const ICMP_INTERVAL: Duration = Duration::from_secs(2);
const ICMP_TIMEOUT_SECONDS: &str = "1";

const CHANNELS: [(&str, &str); 3] = [
    ("red", "LED0_Red"),
    ("green", "LED0_Green"),
    ("blue", "LED0_Blue"),
];

#[derive(Debug)]
pub enum Error {
    InvalidInput(String),
    Hardware(String),
    Config(String),
    Io { context: String, source: io::Error },
}

impl Error {
    fn io(context: impl Into<String>, source: io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(message) => write!(f, "invalid input: {message}"),
            Self::Hardware(message) => write!(f, "unsupported Gale LED hardware: {message}"),
            Self::Config(message) => write!(f, "invalid gale-led configuration: {message}"),
            Self::Io { context, source } => write!(f, "{context}: {source}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Color {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl Color {
    pub fn from_hex(value: &str) -> Result<Self, Error> {
        let Some(digits) = value.strip_prefix('#') else {
            return Err(Error::InvalidInput(
                "color must use the exact #RRGGBB form".into(),
            ));
        };
        let digits = digits.as_bytes();
        if digits.len() != 6 || !digits.iter().all(u8::is_ascii_hexdigit) {
            return Err(Error::InvalidInput(
                "color must contain exactly six hexadecimal digits".into(),
            ));
        }

        let nibble = |digit: u8| match digit {
            b'0'..=b'9' => digit - b'0',
            b'a'..=b'f' => digit - b'a' + 10,
            b'A'..=b'F' => digit - b'A' + 10,
            _ => unreachable!("hex digits were validated above"),
        };
        let byte = |index: usize| (nibble(digits[index]) << 4) | nibble(digits[index + 1]);

        Ok(Self {
            red: byte(0),
            green: byte(2),
            blue: byte(4),
        })
    }

    pub fn from_components(red: u16, green: u16, blue: u16) -> Result<Self, Error> {
        let convert = |name: &str, value: u16| {
            u8::try_from(value).map_err(|_| {
                Error::InvalidInput(format!("{name} channel must be between 0 and 255"))
            })
        };

        Ok(Self {
            red: convert("red", red)?,
            green: convert("green", green)?,
            blue: convert("blue", blue)?,
        })
    }

    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.red, self.green, self.blue)
    }

    pub fn scaled(self, brightness: u8) -> Result<Self, Error> {
        validate_brightness(brightness)?;
        let scale = |value: u8| ((u16::from(value) * u16::from(brightness) + 50) / 100) as u8;
        Ok(Self {
            red: scale(self.red),
            green: scale(self.green),
            blue: scale(self.blue),
        })
    }
}

pub fn validate_brightness(brightness: u8) -> Result<(), Error> {
    if brightness > 100 {
        Err(Error::InvalidInput(
            "brightness must be between 0 and 100".into(),
        ))
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Static,
    Rainbow,
    Breathing,
    NetworkActivity,
    NetworkHeartbeat,
}

impl Mode {
    pub fn parse(value: &str) -> Result<Self, Error> {
        match value {
            "static" => Ok(Self::Static),
            "rainbow" => Ok(Self::Rainbow),
            "breathing" => Ok(Self::Breathing),
            "network_activity" => Ok(Self::NetworkActivity),
            "network_heartbeat" => Ok(Self::NetworkHeartbeat),
            _ => Err(Error::Config(format!(
                "mode must be static, rainbow, breathing, network_activity, or network_heartbeat; got {value}"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Static => "static",
            Self::Rainbow => "rainbow",
            Self::Breathing => "breathing",
            Self::NetworkActivity => "network_activity",
            Self::NetworkHeartbeat => "network_heartbeat",
        }
    }

    pub fn is_dynamic(self) -> bool {
        self != Self::Static
    }

    fn uses_network(self) -> bool {
        matches!(self, Self::NetworkActivity | Self::NetworkHeartbeat)
    }
}

pub fn validate_interface_name(value: &str) -> Result<(), Error> {
    if value.is_empty()
        || value.len() > 15
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.:-".contains(&byte))
    {
        return Err(Error::Config(
            "interface must be 1-15 ASCII letters, digits, dots, underscores, colons, or hyphens"
                .into(),
        ));
    }
    Ok(())
}

pub fn validate_icmp_target(value: &str) -> Result<(), Error> {
    if value.parse::<IpAddr>().is_ok() {
        return Ok(());
    }

    let hostname = value.strip_suffix('.').unwrap_or(value);
    if hostname.is_empty() || value.len() > 253 {
        return Err(Error::Config(
            "ICMP target must be an IPv4 address, IPv6 address, or DNS hostname".into(),
        ));
    }
    let valid = hostname.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    });
    if !valid {
        return Err(Error::Config(
            "ICMP target must be an IPv4 address, IPv6 address, or DNS hostname".into(),
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BehaviorConfig {
    pub mode: Mode,
    pub color: Color,
    pub brightness: u8,
    pub interface: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IcmpConfig {
    pub enabled: bool,
    pub target: String,
    pub failure: BehaviorConfig,
    pub retries: u32,
    pub restore: u32,
}

impl Default for IcmpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            target: "1.1.1.1".into(),
            failure: BehaviorConfig {
                mode: Mode::Static,
                color: Color {
                    red: 255,
                    green: 0,
                    blue: 0,
                },
                brightness: 100,
                interface: "br-lan".into(),
            },
            retries: 3,
            restore: 2,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IcmpState {
    enabled: bool,
    retries: u32,
    restore: u32,
    failed: bool,
    consecutive_failures: u32,
    consecutive_successes: u32,
}

impl IcmpState {
    pub fn new(config: &IcmpConfig) -> Self {
        Self {
            enabled: config.enabled,
            retries: config.retries,
            restore: config.restore,
            failed: false,
            consecutive_failures: 0,
            consecutive_successes: 0,
        }
    }

    pub fn is_failed(&self) -> bool {
        self.failed
    }

    pub fn observe(&mut self, success: bool) -> Option<bool> {
        if !self.enabled {
            return None;
        }

        if self.failed {
            if success {
                self.consecutive_failures = 0;
                self.consecutive_successes = self.consecutive_successes.saturating_add(1);
                if self.consecutive_successes >= self.restore {
                    self.failed = false;
                    self.consecutive_successes = 0;
                    return Some(false);
                }
            } else {
                self.consecutive_successes = 0;
            }
        } else if success {
            self.consecutive_failures = 0;
        } else {
            self.consecutive_successes = 0;
            self.consecutive_failures = self.consecutive_failures.saturating_add(1);
            if self.consecutive_failures >= self.retries {
                self.failed = true;
                self.consecutive_failures = 0;
                return Some(true);
            }
        }
        None
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    pub enabled: bool,
    pub color: Color,
    pub brightness: u8,
    pub mode: Mode,
    pub interface: String,
    pub icmp: IcmpConfig,
}

impl Config {
    pub fn from_values(enabled: &str, color: &str, brightness: &str) -> Result<Self, Error> {
        Self::from_extended_values(enabled, color, brightness, "static", "br-lan")
    }

    pub fn from_extended_values(
        enabled: &str,
        color: &str,
        brightness: &str,
        mode: &str,
        interface: &str,
    ) -> Result<Self, Error> {
        Self::from_all_values(
            enabled, color, brightness, mode, interface, "0", "1.1.1.1", "static", "#FF0000",
            "100", "br-lan", "3", "2",
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn from_all_values(
        enabled: &str,
        color: &str,
        brightness: &str,
        mode: &str,
        interface: &str,
        icmp_enabled: &str,
        icmp_target: &str,
        failure_mode: &str,
        failure_color: &str,
        failure_brightness: &str,
        failure_interface: &str,
        icmp_retries: &str,
        icmp_restore: &str,
    ) -> Result<Self, Error> {
        let enabled = match enabled {
            "1" => true,
            "0" => false,
            _ => return Err(Error::Config("enabled must be 0 or 1".into())),
        };
        let color = Color::from_hex(color).map_err(|error| Error::Config(error.to_string()))?;
        let brightness = brightness.parse::<u8>().map_err(|_| {
            Error::Config("brightness must be an integer from 0 through 100".into())
        })?;
        validate_brightness(brightness).map_err(|error| Error::Config(error.to_string()))?;
        let mode = Mode::parse(mode)?;
        validate_interface_name(interface)?;

        let icmp_enabled = match icmp_enabled {
            "1" => true,
            "0" => false,
            _ => return Err(Error::Config("icmp_enabled must be 0 or 1".into())),
        };
        let parse_threshold = |name: &str, value: &str| -> Result<u32, Error> {
            let value = value
                .parse::<u32>()
                .map_err(|_| Error::Config(format!("{name} must be an integer of at least 1")))?;
            if value == 0 {
                return Err(Error::Config(format!(
                    "{name} must be an integer of at least 1"
                )));
            }
            Ok(value)
        };
        let icmp = if icmp_enabled {
            validate_icmp_target(icmp_target)?;
            let failure_mode = Mode::parse(failure_mode)?;
            let failure_color =
                Color::from_hex(failure_color).map_err(|error| Error::Config(error.to_string()))?;
            let failure_brightness = failure_brightness.parse::<u8>().map_err(|_| {
                Error::Config("failure_brightness must be an integer from 0 through 100".into())
            })?;
            validate_brightness(failure_brightness)
                .map_err(|error| Error::Config(error.to_string()))?;
            validate_interface_name(failure_interface)?;
            IcmpConfig {
                enabled: true,
                target: icmp_target.into(),
                failure: BehaviorConfig {
                    mode: failure_mode,
                    color: failure_color,
                    brightness: failure_brightness,
                    interface: failure_interface.into(),
                },
                retries: parse_threshold("icmp_retries", icmp_retries)?,
                restore: parse_threshold("icmp_restore", icmp_restore)?,
            }
        } else {
            IcmpConfig::default()
        };

        Ok(Self {
            enabled,
            color,
            brightness,
            mode,
            interface: interface.into(),
            icmp,
        })
    }

    pub fn load_uci() -> Result<Self, Error> {
        let get = |option: &str| -> Result<String, Error> {
            let key = format!("gale-led.main.{option}");
            let output = Command::new(UCI_BIN)
                .args(["-q", "get", &key])
                .output()
                .map_err(|error| Error::io(format!("unable to execute {UCI_BIN}"), error))?;
            if !output.status.success() {
                return Err(Error::Config(format!("required option {key} is missing")));
            }
            String::from_utf8(output.stdout)
                .map(|value| value.trim().to_owned())
                .map_err(|_| Error::Config(format!("option {key} is not valid UTF-8")))
        };

        let optional = |option: &str, default: &str| -> Result<String, Error> {
            let key = format!("gale-led.main.{option}");
            let output = Command::new(UCI_BIN)
                .args(["-q", "get", &key])
                .output()
                .map_err(|error| Error::io(format!("unable to execute {UCI_BIN}"), error))?;
            if !output.status.success() {
                return Ok(default.into());
            }
            String::from_utf8(output.stdout)
                .map(|value| value.trim().to_owned())
                .map_err(|_| Error::Config(format!("option {key} is not valid UTF-8")))
        };

        Self::from_all_values(
            &get("enabled")?,
            &get("color")?,
            &get("brightness")?,
            &optional("mode", "static")?,
            &optional("interface", "br-lan")?,
            &optional("icmp_enabled", "0")?,
            &optional("icmp_target", "1.1.1.1")?,
            &optional("failure_mode", "static")?,
            &optional("failure_color", "#FF0000")?,
            &optional("failure_brightness", "100")?,
            &optional("failure_interface", "br-lan")?,
            &optional("icmp_retries", "3")?,
            &optional("icmp_restore", "2")?,
        )
    }

    fn normal_behavior(&self) -> BehaviorConfig {
        BehaviorConfig {
            mode: self.mode,
            color: self.color,
            brightness: self.brightness,
            interface: self.interface.clone(),
        }
    }
}

#[derive(Clone, Debug)]
struct Channel {
    name: &'static str,
    path: PathBuf,
    max_brightness: u32,
}

#[derive(Clone, Debug)]
pub struct Hardware {
    channels: [Channel; 3],
}

impl Hardware {
    pub fn discover() -> Result<Self, Error> {
        Self::discover_at(Path::new(SYSFS_ROOT))
    }

    pub fn discover_at(root: &Path) -> Result<Self, Error> {
        let mut channels = Vec::with_capacity(CHANNELS.len());
        for (name, directory) in CHANNELS {
            let path = root.join(directory);
            if !path.is_dir() {
                return Err(Error::Hardware(format!("missing {}", path.display())));
            }
            for attribute in ["brightness", "max_brightness", "trigger"] {
                let attribute_path = path.join(attribute);
                if !attribute_path.is_file() {
                    return Err(Error::Hardware(format!(
                        "missing {}",
                        attribute_path.display()
                    )));
                }
            }
            let max_brightness = read_u32(&path.join("max_brightness"))?;
            if max_brightness == 0 {
                return Err(Error::Hardware(format!(
                    "{} reports max_brightness 0",
                    path.display()
                )));
            }
            channels.push(Channel {
                name,
                path,
                max_brightness,
            });
        }

        let channels: [Channel; 3] = channels
            .try_into()
            .map_err(|_| Error::Hardware("internal channel definition mismatch".into()))?;
        Ok(Self { channels })
    }

    pub fn apply_config(&self, config: &Config) -> Result<(), Error> {
        if config.enabled {
            self.set(config.color, config.brightness)
        } else {
            self.off()
        }
    }

    pub fn set(&self, color: Color, brightness: u8) -> Result<(), Error> {
        self.prepare()?;
        self.set_prepared(color, brightness)
    }

    pub fn prepare(&self) -> Result<(), Error> {
        for channel in &self.channels {
            write_value(&channel.path.join("trigger"), "none")?;
        }
        Ok(())
    }

    pub fn set_prepared(&self, color: Color, brightness: u8) -> Result<(), Error> {
        let scaled = color.scaled(brightness)?;
        let rgb = [scaled.red, scaled.green, scaled.blue];
        let values = std::array::from_fn::<u32, 3, _>(|index| {
            (u32::from(rgb[index]) * self.channels[index].max_brightness + 127) / 255
        });

        for (channel, value) in self.channels.iter().zip(values) {
            write_value(&channel.path.join("brightness"), &value.to_string())?;
        }
        Ok(())
    }

    pub fn off(&self) -> Result<(), Error> {
        self.set(
            Color {
                red: 0,
                green: 0,
                blue: 0,
            },
            0,
        )
    }

    pub fn hardware_json(&self) -> String {
        let channel = |index: usize| &self.channels[index];
        format!(
            concat!(
                "{{\n",
                "  \"supported\": true,\n",
                "  \"red\": \"{}\",\n",
                "  \"green\": \"{}\",\n",
                "  \"blue\": \"{}\",\n",
                "  \"max_brightness\": {{\"red\": {}, \"green\": {}, \"blue\": {}}}\n",
                "}}"
            ),
            channel(0).path.display(),
            channel(1).path.display(),
            channel(2).path.display(),
            channel(0).max_brightness,
            channel(1).max_brightness,
            channel(2).max_brightness,
        )
    }

    pub fn status_json(&self, config: &Config) -> Result<String, Error> {
        let values = self.current_values()?;
        Ok(format!(
            concat!(
                "{{\n",
                "  \"configured\": {{\"enabled\": {}, \"color\": \"{}\", \"brightness\": {}}},\n",
                "  \"mode\": \"{}\",\n",
                "  \"interface\": \"{}\",\n",
                "  \"icmp\": {{\"enabled\": {}, \"target\": \"{}\", \"failure_mode\": \"{}\", \"retries\": {}, \"restore\": {}}},\n",
                "  \"hardware\": {{\"red\": {}, \"green\": {}, \"blue\": {}}}\n",
                "}}"
            ),
            config.enabled,
            config.color.to_hex(),
            config.brightness,
            config.mode.as_str(),
            config.interface,
            config.icmp.enabled,
            config.icmp.target,
            config.icmp.failure.mode.as_str(),
            config.icmp.retries,
            config.icmp.restore,
            values[0],
            values[1],
            values[2],
        ))
    }

    pub fn current_values(&self) -> Result<[u32; 3], Error> {
        let mut values = [0; 3];
        for (index, channel) in self.channels.iter().enumerate() {
            values[index] = read_u32(&channel.path.join("brightness"))?;
        }
        Ok(values)
    }

    pub fn channel_names(&self) -> [&'static str; 3] {
        std::array::from_fn(|index| self.channels[index].name)
    }
}

pub fn interfaces_json() -> Result<String, Error> {
    interfaces_json_at(Path::new(SYSFS_NET_ROOT))
}

pub fn interfaces_json_at(root: &Path) -> Result<String, Error> {
    let entries = fs::read_dir(root)
        .map_err(|error| Error::io(format!("unable to read {}", root.display()), error))?;
    let mut interfaces = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| Error::io("unable to read network interface", error))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if validate_interface_name(&name).is_err() {
            continue;
        }
        let state = fs::read_to_string(entry.path().join("operstate"))
            .map(|value| value.trim().to_owned())
            .unwrap_or_else(|_| "unknown".into());
        let state = if state.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
            state
        } else {
            "unknown".into()
        };
        interfaces.push((name, state));
    }
    interfaces.sort();
    let values = interfaces
        .iter()
        .map(|(name, state)| format!("{{\"name\":\"{name}\",\"state\":\"{state}\"}}"))
        .collect::<Vec<_>>()
        .join(",");
    Ok(format!("{{\"interfaces\":[{values}]}}"))
}

struct NetworkMonitor {
    path: PathBuf,
}

impl NetworkMonitor {
    fn discover(interface: &str) -> Result<Self, Error> {
        validate_interface_name(interface)?;
        let path = Path::new(SYSFS_NET_ROOT).join(interface);
        if !path.is_dir() {
            return Err(Error::Config(format!(
                "network interface {interface} does not exist"
            )));
        }
        Ok(Self { path })
    }

    fn counters(&self) -> Result<(u64, u64), Error> {
        Ok((
            read_u64(&self.path.join("statistics/rx_bytes"))?,
            read_u64(&self.path.join("statistics/tx_bytes"))?,
        ))
    }

    fn is_up(&self) -> Result<bool, Error> {
        let state = fs::read_to_string(self.path.join("operstate")).map_err(|error| {
            Error::io(
                format!("unable to read {}/operstate", self.path.display()),
                error,
            )
        })?;
        Ok(state.trim() == "up")
    }
}

pub fn validate_runtime(config: &Config) -> Result<(), Error> {
    if config.mode.uses_network() {
        NetworkMonitor::discover(&config.interface)?;
    }
    if config.icmp.enabled && config.icmp.failure.mode.uses_network() {
        NetworkMonitor::discover(&config.icmp.failure.interface)?;
    }
    Ok(())
}

pub fn run(config: &Config, hardware: &Hardware) -> Result<(), Error> {
    if !config.icmp.enabled && (!config.enabled || !config.mode.is_dynamic()) {
        return hardware.apply_config(config);
    }

    validate_runtime(config)?;
    hardware.prepare()?;
    let normal_behavior = config.normal_behavior();
    let failure_behavior = config.icmp.failure.clone();
    let mut active = BehaviorRuntime::new(config.enabled, &normal_behavior)?;
    let results = config
        .icmp
        .enabled
        .then(|| start_icmp_monitor(&config.icmp.target));
    let mut icmp_state = IcmpState::new(&config.icmp);
    let mut last_color = None;

    loop {
        if let Some(results) = &results {
            for success in results.try_iter() {
                if let Some(failed) = icmp_state.observe(success) {
                    if failed {
                        eprintln!("ICMP target entered failure state: {}", config.icmp.target);
                        active = BehaviorRuntime::new(true, &failure_behavior)?;
                    } else {
                        eprintln!("ICMP target recovered: {}", config.icmp.target);
                        active = BehaviorRuntime::new(config.enabled, &normal_behavior)?;
                    }
                    last_color = None;
                }
            }
        }

        let color = active.frame()?;
        if last_color != Some(color) {
            hardware.set_prepared(color, 100)?;
            last_color = Some(color);
        }
        thread::sleep(active.frame_interval());
    }
}

struct BehaviorRuntime {
    enabled: bool,
    behavior: BehaviorConfig,
    network: Option<NetworkMonitor>,
    previous_counters: Option<(u64, u64)>,
    started: Instant,
}

impl BehaviorRuntime {
    fn new(enabled: bool, behavior: &BehaviorConfig) -> Result<Self, Error> {
        let network = if enabled && behavior.mode.uses_network() {
            Some(NetworkMonitor::discover(&behavior.interface)?)
        } else {
            None
        };
        let previous_counters = match &network {
            Some(monitor) if behavior.mode == Mode::NetworkActivity => Some(monitor.counters()?),
            _ => None,
        };
        Ok(Self {
            enabled,
            behavior: behavior.clone(),
            network,
            previous_counters,
            started: Instant::now(),
        })
    }

    fn frame(&mut self) -> Result<Color, Error> {
        if !self.enabled {
            return Ok(Color {
                red: 0,
                green: 0,
                blue: 0,
            });
        }

        let elapsed_ms = self.started.elapsed().as_millis() as u64;
        match self.behavior.mode {
            Mode::Static => self.behavior.color.scaled(self.behavior.brightness),
            Mode::Rainbow => rainbow_color(elapsed_ms, self.behavior.brightness),
            Mode::Breathing => self.behavior.color.scaled(effect_brightness(
                self.behavior.brightness,
                breathing_level(elapsed_ms),
            )),
            Mode::NetworkActivity => {
                let monitor = self.network.as_ref().ok_or_else(|| {
                    Error::Config("network activity mode requires an interface".into())
                })?;
                let counters = monitor.counters()?;
                let has_activity = self
                    .previous_counters
                    .is_some_and(|previous| previous != counters);
                self.previous_counters = Some(counters);
                self.behavior.color.scaled(if has_activity {
                    self.behavior.brightness
                } else {
                    0
                })
            }
            Mode::NetworkHeartbeat => {
                let monitor = self.network.as_ref().ok_or_else(|| {
                    Error::Config("network heartbeat mode requires an interface".into())
                })?;
                let level = if monitor.is_up()? {
                    heartbeat_level(elapsed_ms)
                } else {
                    0
                };
                self.behavior
                    .color
                    .scaled(effect_brightness(self.behavior.brightness, level))
            }
        }
    }

    fn frame_interval(&self) -> Duration {
        Duration::from_millis(match self.behavior.mode {
            Mode::NetworkActivity | Mode::NetworkHeartbeat => 100,
            _ => 50,
        })
    }
}

fn start_icmp_monitor(target: &str) -> Receiver<bool> {
    let (sender, receiver) = mpsc::channel();
    let target = target.to_owned();
    thread::spawn(move || {
        loop {
            if sender.send(icmp_echo(&target)).is_err() {
                return;
            }
            thread::sleep(ICMP_INTERVAL);
        }
    });
    receiver
}

fn icmp_echo(target: &str) -> bool {
    icmp_echo_with(PING_BIN, target)
}

fn icmp_echo_with(binary: &str, target: &str) -> bool {
    let mut command = Command::new(binary);
    command.args(["-n", "-c", "1", "-W", ICMP_TIMEOUT_SECONDS]);
    if target.parse::<Ipv6Addr>().is_ok() {
        command.arg("-6");
    }
    command
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn effect_brightness(maximum: u8, level: u8) -> u8 {
    ((u16::from(maximum) * u16::from(level) + 50) / 100) as u8
}

pub fn breathing_level(elapsed_ms: u64) -> u8 {
    let phase = (elapsed_ms % 4000) as f64 / 4000.0;
    ((1.0 - (phase * std::f64::consts::TAU).cos()) * 47.5 + 5.0).round() as u8
}

pub fn heartbeat_level(elapsed_ms: u64) -> u8 {
    fn pulse(phase: u64, start: u64, duration: u64, peak: u8) -> u8 {
        if phase < start || phase >= start + duration {
            return 0;
        }
        let position = phase - start;
        let half = duration / 2;
        let distance = if position <= half {
            position
        } else {
            duration - position
        };
        ((u64::from(peak) * distance + half / 2) / half) as u8
    }

    let phase = elapsed_ms % 2000;
    pulse(phase, 0, 240, 100).max(pulse(phase, 320, 220, 70))
}

pub fn rainbow_color(elapsed_ms: u64, brightness: u8) -> Result<Color, Error> {
    let position = ((elapsed_ms % 6000) * 1536 / 6000) as u16;
    let segment = position / 256;
    let offset = (position % 256) as u8;
    let inverse = 255 - offset;
    let color = match segment {
        0 => Color {
            red: 255,
            green: offset,
            blue: 0,
        },
        1 => Color {
            red: inverse,
            green: 255,
            blue: 0,
        },
        2 => Color {
            red: 0,
            green: 255,
            blue: offset,
        },
        3 => Color {
            red: 0,
            green: inverse,
            blue: 255,
        },
        4 => Color {
            red: offset,
            green: 0,
            blue: 255,
        },
        _ => Color {
            red: 255,
            green: 0,
            blue: inverse,
        },
    };
    color.scaled(brightness)
}

fn read_u32(path: &Path) -> Result<u32, Error> {
    let value = fs::read_to_string(path)
        .map_err(|error| Error::io(format!("unable to read {}", path.display()), error))?;
    value.trim().parse::<u32>().map_err(|_| {
        Error::Hardware(format!(
            "{} does not contain a valid non-negative integer",
            path.display()
        ))
    })
}

fn read_u64(path: &Path) -> Result<u64, Error> {
    let value = fs::read_to_string(path)
        .map_err(|error| Error::io(format!("unable to read {}", path.display()), error))?;
    value.trim().parse::<u64>().map_err(|_| {
        Error::Hardware(format!(
            "{} does not contain a valid non-negative integer",
            path.display()
        ))
    })
}

fn write_value(path: &Path, value: &str) -> Result<(), Error> {
    fs::write(path, value)
        .map_err(|error| Error::io(format!("unable to write {}", path.display()), error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock should be after Unix epoch")
                .as_nanos();
            let root =
                std::env::temp_dir().join(format!("gale-led-test-{}-{nonce}", std::process::id()));
            fs::create_dir(&root).expect("fixture root should be created");
            Self { root }
        }

        fn populate(&self) {
            for (_, directory) in CHANNELS {
                let path = self.root.join(directory);
                fs::create_dir(&path).expect("channel directory should be created");
                fs::write(path.join("brightness"), "0\n")
                    .expect("brightness fixture should be written");
                fs::write(path.join("max_brightness"), "255\n")
                    .expect("max fixture should be written");
                fs::write(path.join("trigger"), "timer\n")
                    .expect("trigger fixture should be written");
                fs::write(path.join("led_current"), "100\n")
                    .expect("current fixture should be written");
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn parses_hex_colors_case_insensitively() {
        assert_eq!(
            Color::from_hex("#A020f0").unwrap(),
            Color {
                red: 160,
                green: 32,
                blue: 240
            }
        );
    }

    #[test]
    fn rejects_malformed_hex_colors() {
        for invalid in [
            "A020F0", "#A020F", "#A020F00", "#GG20F0", "#123 56", "#é1234",
        ] {
            assert!(Color::from_hex(invalid).is_err(), "{invalid} should fail");
        }
    }

    #[test]
    fn rejects_rgb_values_above_255() {
        assert!(Color::from_components(256, 0, 0).is_err());
        assert!(Color::from_components(0, 999, 0).is_err());
    }

    #[test]
    fn scales_rgb_with_rounding() {
        let color = Color {
            red: 255,
            green: 127,
            blue: 1,
        };
        assert_eq!(
            color.scaled(50).unwrap(),
            Color {
                red: 128,
                green: 64,
                blue: 1
            }
        );
        assert_eq!(
            color.scaled(0).unwrap(),
            Color {
                red: 0,
                green: 0,
                blue: 0
            }
        );
        assert!(color.scaled(101).is_err());
    }

    #[test]
    fn validates_config_values() {
        assert_eq!(
            Config::from_values("1", "#A020F0", "75").unwrap(),
            Config {
                enabled: true,
                color: Color {
                    red: 160,
                    green: 32,
                    blue: 240
                },
                brightness: 75,
                mode: Mode::Static,
                interface: "br-lan".into(),
                icmp: IcmpConfig {
                    enabled: false,
                    target: "1.1.1.1".into(),
                    failure: BehaviorConfig {
                        mode: Mode::Static,
                        color: Color {
                            red: 255,
                            green: 0,
                            blue: 0,
                        },
                        brightness: 100,
                        interface: "br-lan".into(),
                    },
                    retries: 3,
                    restore: 2,
                },
            }
        );
        assert!(Config::from_values("yes", "#A020F0", "75").is_err());
        assert!(Config::from_values("1", "#A020F0", "101").is_err());
        assert!(Config::from_values("1", "purple", "75").is_err());
        assert!(Config::from_extended_values("1", "#A020F0", "75", "unknown", "br-lan").is_err());
        assert!(
            Config::from_extended_values("1", "#A020F0", "75", "network_activity", "../wan")
                .is_err()
        );
    }

    fn icmp_test_config(enabled: bool, retries: u32, restore: u32) -> IcmpConfig {
        IcmpConfig {
            enabled,
            target: "1.1.1.1".into(),
            failure: BehaviorConfig {
                mode: Mode::Static,
                color: Color {
                    red: 255,
                    green: 0,
                    blue: 0,
                },
                brightness: 100,
                interface: "br-lan".into(),
            },
            retries,
            restore,
        }
    }

    #[test]
    fn disabled_icmp_ignores_results_and_preserves_static_apply() {
        let mut state = IcmpState::new(&icmp_test_config(false, 1, 1));
        assert_eq!(state.observe(false), None);
        assert!(!state.is_failed());

        let fixture = Fixture::new();
        fixture.populate();
        let hardware = Hardware::discover_at(&fixture.root).unwrap();
        let config = Config::from_values("1", "#A020F0", "75").unwrap();
        run(&config, &hardware).unwrap();
        assert_eq!(hardware.current_values().unwrap(), [120, 24, 180]);
    }

    #[test]
    fn successful_icmp_keeps_normal_state() {
        let mut state = IcmpState::new(&icmp_test_config(true, 3, 2));
        assert_eq!(state.observe(true), None);
        assert!(!state.is_failed());
    }

    #[test]
    fn icmp_failure_activates_only_at_retry_threshold() {
        let mut state = IcmpState::new(&icmp_test_config(true, 3, 2));
        assert_eq!(state.observe(false), None);
        assert_eq!(state.observe(false), None);
        assert!(!state.is_failed());
        assert_eq!(state.observe(false), Some(true));
        assert!(state.is_failed());
    }

    #[test]
    fn icmp_recovery_activates_only_at_restore_threshold() {
        let mut state = IcmpState::new(&icmp_test_config(true, 1, 2));
        assert_eq!(state.observe(false), Some(true));
        assert_eq!(state.observe(true), None);
        assert!(state.is_failed());
        assert_eq!(state.observe(true), Some(false));
        assert!(!state.is_failed());
    }

    #[test]
    fn icmp_counters_reset_when_result_direction_changes() {
        let mut state = IcmpState::new(&icmp_test_config(true, 3, 2));
        assert_eq!(state.observe(false), None);
        assert_eq!(state.observe(false), None);
        assert_eq!(state.observe(true), None);
        assert_eq!(state.observe(false), None);
        assert_eq!(state.observe(false), None);
        assert!(!state.is_failed());
        assert_eq!(state.observe(false), Some(true));

        assert_eq!(state.observe(true), None);
        assert_eq!(state.observe(false), None);
        assert_eq!(state.observe(true), None);
        assert!(state.is_failed());
        assert_eq!(state.observe(true), Some(false));
    }

    #[test]
    fn validates_icmp_targets_and_probe_errors_are_failures() {
        for valid in ["1.1.1.1", "2606:4700:4700::1111", "one.one.one.one."] {
            validate_icmp_target(valid).unwrap();
        }
        for invalid in ["", "-option", "bad target", "bad..target"] {
            assert!(validate_icmp_target(invalid).is_err());
        }
        assert!(!icmp_echo_with(
            "/definitely/not/a/ping-binary",
            "unresolvable.invalid"
        ));
    }

    #[test]
    fn existing_configuration_receives_icmp_defaults() {
        let config =
            Config::from_extended_values("1", "#A020F0", "75", "rainbow", "br-lan").unwrap();
        assert!(!config.icmp.enabled);
        assert_eq!(config.icmp.target, "1.1.1.1");
        assert_eq!(config.icmp.failure.mode, Mode::Static);
        assert_eq!(config.icmp.failure.color.to_hex(), "#FF0000");
        assert_eq!(config.icmp.retries, 3);
        assert_eq!(config.icmp.restore, 2);
    }

    #[test]
    fn disabled_icmp_does_not_validate_hidden_settings() {
        let config = Config::from_all_values(
            "1",
            "#A020F0",
            "75",
            "static",
            "br-lan",
            "0",
            "not a valid target",
            "not_a_mode",
            "not-a-color",
            "not-a-number",
            "not an interface",
            "0",
            "0",
        )
        .unwrap();
        assert_eq!(config.icmp, IcmpConfig::default());
    }

    #[test]
    fn computes_effect_frames() {
        assert_eq!(breathing_level(0), 5);
        assert_eq!(breathing_level(2000), 100);
        assert_eq!(heartbeat_level(0), 0);
        assert_eq!(heartbeat_level(120), 100);
        assert_eq!(heartbeat_level(430), 70);
        assert_eq!(heartbeat_level(1000), 0);
        assert_eq!(
            rainbow_color(0, 100).unwrap(),
            Color {
                red: 255,
                green: 0,
                blue: 0
            }
        );
        assert_eq!(
            rainbow_color(1000, 50).unwrap(),
            Color {
                red: 128,
                green: 128,
                blue: 0
            }
        );
        assert!(rainbow_color(0, 101).is_err());
    }

    #[test]
    fn lists_safe_network_interfaces() {
        let fixture = Fixture::new();
        let interface = fixture.root.join("br-lan");
        fs::create_dir(&interface).unwrap();
        fs::write(interface.join("operstate"), "up\n").unwrap();
        fs::create_dir(fixture.root.join("bad interface")).unwrap();

        assert_eq!(
            interfaces_json_at(&fixture.root).unwrap(),
            "{\"interfaces\":[{\"name\":\"br-lan\",\"state\":\"up\"}]}"
        );
    }

    #[test]
    fn reports_missing_hardware_without_writes() {
        let fixture = Fixture::new();
        let error = Hardware::discover_at(&fixture.root).unwrap_err();
        assert!(error.to_string().contains("LED0_Red"));
    }

    #[test]
    fn applies_scaled_color_and_preserves_led_current() {
        let fixture = Fixture::new();
        fixture.populate();
        let hardware = Hardware::discover_at(&fixture.root).unwrap();
        hardware
            .set(
                Color {
                    red: 160,
                    green: 32,
                    blue: 240,
                },
                75,
            )
            .unwrap();

        assert_eq!(hardware.current_values().unwrap(), [120, 24, 180]);
        assert_eq!(hardware.channel_names(), ["red", "green", "blue"]);
        for (_, directory) in CHANNELS {
            let path = fixture.root.join(directory);
            assert_eq!(fs::read_to_string(path.join("trigger")).unwrap(), "none");
            assert_eq!(
                fs::read_to_string(path.join("led_current")).unwrap(),
                "100\n"
            );
        }
    }
}
