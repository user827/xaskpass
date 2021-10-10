use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use toml::Value;

use crate::bail;
use crate::errors::{Context as _, Error, Result};

pub const NAME: &str = env!("CARGO_PKG_NAME");

pub struct Loader {
    pub xdg_dirs: xdg::BaseDirectories,
}
impl Loader {
    pub fn new() -> Self {
        let xdg_dirs = xdg::BaseDirectories::with_prefix(NAME).unwrap();
        Self { xdg_dirs }
    }

    pub fn load(&self) -> Result<Config> {
        self.xdg_dirs
            .find_config_file(format!("{}.toml", NAME))
            .as_deref()
            .map_or_else(|| Ok(Config::default()), Self::load_path)
    }

    pub fn load_path(path: &Path) -> Result<Config> {
        let data = std::fs::read_to_string(&path).context("Config file")?;
        Ok(toml::from_str(&data).context("Config Toml")?)
    }

    pub fn print(cfg: &Config) -> Result<()> {
        let toml = toml::to_string_pretty(cfg).context("toml serialize")?;
        std::io::stdout()
            .write_all(toml.as_bytes())
            .expect("Unable to write data");
        Ok(())
    }
}

pub fn option_explicit_none<'de, T, D>(deserializer: D) -> std::result::Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(match Value::deserialize(deserializer)? {
        Value::String(ref value) if value.to_lowercase() == "none" => None,
        value => Some(T::deserialize(value).map_err(serde::de::Error::custom)?),
    })
}

pub fn option_explicit_serialize<T, S>(
    val: &Option<T>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: Serializer,
    T: Serialize,
{
    match val {
        None => str::serialize("none", serializer),
        Some(ref val) => T::serialize(val, serializer),
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Rgba {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub alpha: u8,
}

impl Serialize for Rgba {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let hex = if self.alpha == u8::MAX {
            hex::encode([self.red, self.green, self.blue])
        } else {
            hex::encode([self.red, self.green, self.blue, self.alpha])
        };
        serializer.serialize_str(&format!("#{}", hex))
    }
}

impl<'de> Deserialize<'de> for Rgba {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl std::str::FromStr for Rgba {
    type Err = Error;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        log::trace!("rgba::from_str {}", s);
        let without_prefix = s.trim_start_matches('#');
        match without_prefix.len() {
            8 => {
                let mut bytes = [0_u8; 4];
                hex::decode_to_slice(without_prefix, &mut bytes).context("color")?;
                log::trace!("rgba::from_str {:?}", bytes);
                Ok(Rgba {
                    red: bytes[0],
                    green: bytes[1],
                    blue: bytes[2],
                    alpha: bytes[3],
                })
            }
            6 => {
                let mut bytes = [0_u8; 3];
                hex::decode_to_slice(without_prefix, &mut bytes).context("color")?;
                log::trace!("rgba::from_str {:?}", bytes);
                Ok(Rgba {
                    red: bytes[0],
                    green: bytes[1],
                    blue: bytes[2],
                    alpha: u8::MAX,
                })
            }
            n => bail!("invalid hex color length: {}", n),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub title: String,
    pub grab_keyboard: bool,
    pub show_hostname: bool,
    pub resizable: bool,
    pub depth: u8,
    pub dialog: Dialog,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            title: NAME.into(),
            grab_keyboard: false,
            show_hostname: false,
            resizable: false,
            depth: 32,
            dialog: Dialog::default(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Dialog {
    #[serde(serialize_with = "option_explicit_serialize")]
    #[serde(deserialize_with = "option_explicit_none")]
    pub font: Option<String>,
    #[serde(serialize_with = "option_explicit_serialize")]
    #[serde(deserialize_with = "option_explicit_none")]
    pub font_file: Option<std::ffi::CString>,
    #[serde(serialize_with = "option_explicit_serialize")]
    #[serde(deserialize_with = "option_explicit_none")]
    pub direction: Option<PangoDirection>,
    pub label: String,
    pub alignment: PangoAlignment,
    #[serde(serialize_with = "option_explicit_serialize")]
    #[serde(deserialize_with = "option_explicit_none")]
    pub scale: Option<f64>,
    pub indicator_label: String,
    #[serde(serialize_with = "option_explicit_serialize")]
    #[serde(deserialize_with = "option_explicit_none")]
    pub input_timeout: Option<u64>,
    pub foreground: Rgba,
    pub indicator_label_foreground: Rgba,
    pub background: Rgba,
    pub layout_opts: Layout,
    pub ok_button: TextButton,
    pub cancel_button: TextButton,
    pub clipboard_button: ClipboardButton,
    pub plaintext_button: TextButton,
    pub indicator: Indicator,
}

impl Default for Dialog {
    fn default() -> Self {
        let button = Button::default();
        let ok_button = TextButton {
            label: "OK".into(),
            foreground: "#5c616c".parse().unwrap(),
            button: button.clone(),
        };
        let cancel_button = TextButton {
            label: "Cancel".into(),
            ..ok_button.clone()
        };

        let plaintext_button = TextButton {
            label: "abc".into(),
            ..ok_button.clone()
        };

        Self {
            foreground: "#5c616c".parse().unwrap(),
            indicator_label_foreground: "#5c616c".parse().unwrap(),
            background: "#f5f6f7ee".parse().unwrap(),
            label: "Please enter your authentication passphrase:".into(),
            alignment: PangoAlignment::Center,
            indicator_label: "Secret:".into(),
            input_timeout: Some(30),
            font: Some("11".into()),
            direction: None,
            scale: None,
            font_file: None,
            layout_opts: Layout::default(),
            ok_button,
            cancel_button,
            plaintext_button,
            clipboard_button: ClipboardButton {
                foreground: "#5c616c".parse().unwrap(),
                button,
            },
            indicator: Indicator::default(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipboardButton {
    pub foreground: Rgba,
    #[serde(flatten)]
    pub button: Button,
}

impl Default for ClipboardButton {
    fn default() -> Self {
        Self {
            foreground: "#5c616c".parse().unwrap(),
            button: Button::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TextButton {
    pub label: String,
    pub foreground: Rgba,
    #[serde(flatten)]
    pub button: Button,
}

impl Default for TextButton {
    fn default() -> Self {
        Self {
            label: "label".into(),
            foreground: "#5c616c".parse().unwrap(),
            button: Button::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Button {
    #[serde(deserialize_with = "option_explicit_none")]
    #[serde(serialize_with = "option_explicit_serialize")]
    pub horizontal_spacing: Option<f64>,
    #[serde(serialize_with = "option_explicit_serialize")]
    #[serde(deserialize_with = "option_explicit_none")]
    pub vertical_spacing: Option<f64>,
    pub border_width: f64,
    pub radius_x: f64,
    pub radius_y: f64,
    pub pressed_adjustment_x: f64,
    pub pressed_adjustment_y: f64,
    pub background: Rgba,
    pub border_color: Rgba,
    pub border_color_pressed: Rgba,
    #[serde(serialize_with = "option_explicit_serialize")]
    #[serde(deserialize_with = "option_explicit_none")]
    pub background_stop: Option<Rgba>,
    pub background_pressed: Rgba,
    #[serde(serialize_with = "option_explicit_serialize")]
    #[serde(deserialize_with = "option_explicit_none")]
    pub background_pressed_stop: Option<Rgba>,
    #[serde(serialize_with = "option_explicit_serialize")]
    #[serde(deserialize_with = "option_explicit_none")]
    pub background_hover_stop: Option<Rgba>,
    pub background_hover: Rgba,
}

impl Default for Button {
    fn default() -> Self {
        Self {
            background: "#fcfdfd".parse().unwrap(),
            background_stop: None,
            background_pressed: "#d3d8e2".parse().unwrap(),
            background_pressed_stop: None,
            background_hover: "#ffffff".parse().unwrap(),
            background_hover_stop: None,
            horizontal_spacing: None,
            vertical_spacing: None,
            border_width: 1.0,
            border_color: "#cfd6e6".parse().unwrap(),
            border_color_pressed: "#b7c0d3".parse().unwrap(),
            radius_x: 2.0,
            radius_y: 2.0,
            pressed_adjustment_x: 1.0,
            pressed_adjustment_y: 1.0,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    pub layout: crate::dialog::layout::Layout,
    #[serde(serialize_with = "option_explicit_serialize")]
    #[serde(deserialize_with = "option_explicit_none")]
    horizontal_spacing: Option<f64>,
    #[serde(serialize_with = "option_explicit_serialize")]
    #[serde(deserialize_with = "option_explicit_none")]
    vertical_spacing: Option<f64>,
    #[serde(serialize_with = "option_explicit_serialize")]
    #[serde(deserialize_with = "option_explicit_none")]
    pub text_width: Option<u32>,
}

impl Layout {
    pub fn horizontal_spacing(&self, text_height: f64) -> f64 {
        self.horizontal_spacing
            .unwrap_or_else(|| (text_height / 1.7).round())
    }
    pub fn vertical_spacing(&self, text_height: f64) -> f64 {
        self.vertical_spacing
            .unwrap_or_else(|| (text_height / 1.7).round())
    }
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            layout: crate::dialog::layout::Layout::Center,
            horizontal_spacing: None,
            vertical_spacing: None,
            text_width: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Copy)]
#[serde(default)]
pub struct IndicatorClassic {
    pub min_count: u16,
    pub max_count: u16,
    pub radius_x: f64,
    pub radius_y: f64,
    #[serde(deserialize_with = "option_explicit_none")]
    #[serde(serialize_with = "option_explicit_serialize")]
    pub horizontal_spacing: Option<f64>,
    #[serde(serialize_with = "option_explicit_serialize")]
    #[serde(deserialize_with = "option_explicit_none")]
    pub element_height: Option<f64>,
    #[serde(serialize_with = "option_explicit_serialize")]
    #[serde(deserialize_with = "option_explicit_none")]
    pub element_width: Option<f64>,
}

impl Default for IndicatorClassic {
    fn default() -> Self {
        Self {
            min_count: 3,
            max_count: 3,
            radius_x: 2.0,
            radius_y: 2.0,
            horizontal_spacing: None,
            element_height: None,
            element_width: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Copy)]
#[serde(default)]
pub struct IndicatorCircle {
    #[serde(serialize_with = "option_explicit_serialize")]
    #[serde(deserialize_with = "option_explicit_none")]
    pub diameter: Option<f64>,
    pub rotate: bool,
    pub rotation_speed_start: f64,
    pub rotation_speed_gain: f64,
    pub light_up: bool,
    pub spacing_angle: f64,
    pub indicator_count: u32,
    #[serde(serialize_with = "option_explicit_serialize")]
    #[serde(deserialize_with = "option_explicit_none")]
    pub indicator_width: Option<f64>,
    pub lock_color: Rgba,
}

impl Default for IndicatorCircle {
    fn default() -> Self {
        Self {
            diameter: None,
            rotate: true,
            light_up: true,
            rotation_speed_start: 0.10,
            rotation_speed_gain: 1.05,
            spacing_angle: 0.5,
            indicator_count: 3,
            indicator_width: None,
            lock_color: "#ffffff".parse().unwrap(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Disco {
    pub min_count: u16,
    pub max_count: u16,
    pub three_states: bool,
}

impl Default for Disco {
    fn default() -> Self {
        Self {
            min_count: 3,
            max_count: 3,
            three_states: false,
        }
    }
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
pub enum PangoAlignment {
    Left,
    Center,
    Right,
}

impl From<PangoAlignment> for pango::Alignment {
    fn from(val: PangoAlignment) -> Self {
        match val {
            PangoAlignment::Left => Self::Left,
            PangoAlignment::Center => Self::Center,
            PangoAlignment::Right => Self::Right,
        }
    }
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
pub enum PangoDirection {
    Ltr,
    Neutral,
    Rtl,
    WeakLtr,
    WeakRtl,
}

impl From<PangoDirection> for pango::Direction {
    fn from(val: PangoDirection) -> Self {
        match val {
            PangoDirection::Ltr => Self::Ltr,
            PangoDirection::Neutral => Self::Neutral,
            PangoDirection::Rtl => Self::Rtl,
            PangoDirection::WeakLtr => Self::WeakLtr,
            PangoDirection::WeakRtl => Self::WeakRtl,
        }
    }
}

fn strings<'de, D>(d: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::de::Deserializer<'de>,
{
    let arr: Vec<String> = Vec::deserialize(d)?;

    if arr.len() < 2 {
        return Err(serde::de::Error::custom(
            "strings should have at least 2 elements",
        ));
    }

    Ok(arr)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Custom {
    pub alignment: PangoAlignment,
    pub justify: bool,
    pub randomize: bool,
    #[serde(deserialize_with = "strings")]
    pub strings: Vec<String>,
}

#[allow(clippy::unicode_not_nfc)]
impl Default for Custom {
    fn default() -> Self {
        Self {
            alignment: PangoAlignment::Center,
            justify: false,
            randomize: true,
            strings: vec![
                "pasted 🤯".into(),
                "(っ-̶●̃益●̶̃)っ ,︵‿ ".into(),
                "(⊙.⊙(☉̃ₒ☉)⊙.⊙)".into(),
                "ʕ•́ᴥ•̀ʔっ".into(),
                "ヽ(´ー`)人(´∇｀)人(`Д´)ノ".into(),
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "strings")]
pub enum StringType {
    Disco {
        #[serde(default)]
        disco: Disco,
    },
    Custom {
        #[serde(default)]
        custom: Custom,
    },
    Asterisk {
        #[serde(default)]
        asterisk: Asterisk,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IndicatorStrings {
    #[serde(serialize_with = "option_explicit_serialize")]
    #[serde(deserialize_with = "option_explicit_none")]
    pub horizontal_spacing: Option<f64>,
    #[serde(serialize_with = "option_explicit_serialize")]
    #[serde(deserialize_with = "option_explicit_none")]
    pub vertical_spacing: Option<f64>,
    pub radius_x: f64,
    pub radius_y: f64,
    #[serde(flatten)]
    pub strings: StringType,
}

impl Default for IndicatorStrings {
    fn default() -> Self {
        Self {
            horizontal_spacing: None,
            vertical_spacing: None,
            radius_x: 2.0,
            radius_y: 2.0,
            strings: StringType::Asterisk {
                asterisk: Asterisk::default(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IndicatorType {
    Strings {
        #[serde(default)]
        strings: IndicatorStrings,
    },
    Circle {
        #[serde(default)]
        circle: IndicatorCircle,
    },
    Classic {
        #[serde(default)]
        classic: IndicatorClassic,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Indicator {
    #[serde(flatten)]
    pub common: IndicatorCommon,
    #[serde(rename = "type")]
    #[serde(flatten)]
    pub indicator_type: IndicatorType,
}

impl Default for Indicator {
    fn default() -> Self {
        Self {
            indicator_type: IndicatorType::Circle {
                circle: IndicatorCircle::default(),
            },
            common: IndicatorCommon::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Copy)]
#[serde(default)]
pub struct IndicatorCommon {
    pub border_width: f64,
    pub blink: bool,
    pub foreground: Rgba,
    pub background: Rgba,
    #[serde(deserialize_with = "option_explicit_none")]
    #[serde(serialize_with = "option_explicit_serialize")]
    pub background_stop: Option<Rgba>,
    pub border_color: Rgba,
    pub border_color_focused: Rgba,
    pub indicator_color: Rgba,
    #[serde(deserialize_with = "option_explicit_none")]
    #[serde(serialize_with = "option_explicit_serialize")]
    pub indicator_color_stop: Option<Rgba>,
}

impl Default for IndicatorCommon {
    fn default() -> Self {
        Self {
            border_width: 1.0,
            foreground: "#5c616c".parse().unwrap(),
            background: "#ffffff".parse().unwrap(),
            background_stop: None,
            blink: true,
            border_color: "#cfd6e6".parse().unwrap(),
            border_color_focused: "#5294e2".parse().unwrap(),
            indicator_color: "#d3d8e2".parse().unwrap(),
            indicator_color_stop: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Asterisk {
    pub alignment: PangoAlignment,
    pub asterisk: String,
    pub min_count: u16,
    pub max_count: u16,
}

impl Default for Asterisk {
    fn default() -> Self {
        Self {
            alignment: PangoAlignment::Center,
            asterisk: "*".into(),
            min_count: 10,
            max_count: 20,
        }
    }
}
