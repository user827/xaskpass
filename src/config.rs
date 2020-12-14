use std::path::Path;

use anyhow::Context as _;
use color_processing::Color;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use toml::Value;

pub const NAME: &str = env!("CARGO_PKG_NAME");

pub struct Loader {
    pub xdg_dirs: xdg::BaseDirectories,
}
impl Loader {
    pub fn new() -> Self {
        let xdg_dirs = xdg::BaseDirectories::with_prefix(NAME).unwrap();
        Self { xdg_dirs }
    }

    pub fn load(&self) -> Result<Config, anyhow::Error> {
        if let Some(path) = self.xdg_dirs.find_config_file(format!("{}.toml", NAME)) {
            self.load_path(&path)
        } else {
            Ok(Config::default())
        }
    }

    pub fn load_path(&self, path: &Path) -> Result<Config, anyhow::Error> {
        let data = std::fs::read_to_string(&path).context("Config file")?;
        Ok(toml::from_str(&data).context("Config Toml")?)
    }

    pub fn save_path(&self, path: &Path, cfg: &Config) -> Result<(), anyhow::Error> {
        let toml = toml::to_string_pretty(cfg).context("toml serialize")?;
        std::fs::write(path, toml).context("write config")?;
        Ok(())
    }
}

pub fn option_explicit_none<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(match Value::deserialize(deserializer)? {
        Value::String(ref value) if value.to_lowercase() == "none" => None,
        value => Some(T::deserialize(value).map_err(serde::de::Error::custom)?),
    })
}

#[derive(Debug, Clone)]
pub struct Rgba(pub Color);

impl Serialize for Rgba {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.get_original_string())
    }
}

impl<'de> Deserialize<'de> for Rgba {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(s.parse().map_err(serde::de::Error::custom)?)
    }
}

impl std::str::FromStr for Rgba {
    type Err = color_processing::ParseError;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(Rgba(Color::new_string(s)?))
    }
}

impl std::ops::Deref for Rgba {
    type Target = Color;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub title: String,
    #[serde(deserialize_with = "option_explicit_none")]
    pub input_timeout: Option<u64>,
    pub grab_keyboard: bool,
    pub depth: u8,
    pub label: String,
    pub dialog: Dialog,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            title: "Passphrase request".into(),
            input_timeout: Some(30),
            grab_keyboard: false,
            label: "Please enter your authentication passphrase:".into(),
            depth: 24,
            dialog: Dialog::default(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Dialog {
    #[serde(deserialize_with = "option_explicit_none")]
    pub dpi: Option<f64>,
    pub font: String,
    pub foreground: Rgba,
    pub background: Rgba,
    pub layout_opts: Layout,
    pub ok_button: Button,
    pub cancel_button: Button,
    pub indicator: Indicator,
}

impl Default for Dialog {
    fn default() -> Self {
        let ok_button = Button {
            label: "OK".into(),
            ..Button::default()
        };
        let cancel_button = Button {
            label: "Cancel".into(),
            ..ok_button.clone()
        };
        Self {
            foreground: "#5c616c".parse().unwrap(),
            background: "#f5f6f7".parse().unwrap(),
            dpi: None,
            font: "sans serif 11".into(),
            layout_opts: Layout::default(),
            ok_button,
            cancel_button,
            indicator: Indicator::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Button {
    pub label: String,
    pub horizontal_spacing: f64,
    pub vertical_spacing: f64,
    pub border_width: f64,
    pub radius_x: f64,
    pub radius_y: f64,
    pub pressed_adjustment_x: f64,
    pub pressed_adjustment_y: f64,
    pub background: Rgba,
    pub foreground: Rgba,
    pub border_color: Rgba,
    #[serde(deserialize_with = "option_explicit_none")]
    pub background_stop: Option<Rgba>,
    pub background_pressed: Rgba,
    #[serde(deserialize_with = "option_explicit_none")]
    pub background_pressed_stop: Option<Rgba>,
    #[serde(deserialize_with = "option_explicit_none")]
    pub background_hover_stop: Option<Rgba>,
    pub background_hover: Rgba,
}

impl Default for Button {
    fn default() -> Self {
        Self {
            label: "label".into(),
            foreground: "#5c616c".parse().unwrap(),
            background: "#fcfdfd".parse().unwrap(),
            background_stop: None,
            background_pressed: "#d3d8e2".parse().unwrap(),
            background_pressed_stop: None,
            background_hover: "#ffffff".parse().unwrap(),
            background_hover_stop: None,
            horizontal_spacing: 16.0,
            vertical_spacing: 7.0,
            border_width: 1.0,
            border_color: "#cfd6e6".parse().unwrap(),
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
    pub horizontal_spacing: f64,
    pub vertical_spacing: f64,
    #[serde(deserialize_with = "option_explicit_none")]
    pub text_width: Option<u32>,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            layout: crate::dialog::layout::Layout::Center,
            horizontal_spacing: 10.0,
            vertical_spacing: 10.0,
            text_width: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IndicatorClassic {
    pub min_count: u16,
    pub max_count: u16,
    pub radius_x: f64,
    pub radius_y: f64,
    #[serde(deserialize_with = "option_explicit_none")]
    pub horizontal_spacing: Option<f64>,
    #[serde(deserialize_with = "option_explicit_none")]
    pub element_height: Option<f64>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IndicatorCircle {
    #[serde(deserialize_with = "option_explicit_none")]
    pub diameter: Option<f64>,
    pub spacing_angle: f64,
    pub indicator_count: u32,
    #[serde(deserialize_with = "option_explicit_none")]
    pub indicator_width: Option<f64>,
    pub lock_color: Rgba,
}

impl Default for IndicatorCircle {
    fn default() -> Self {
        Self {
            diameter: None,
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


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "strings")]
pub enum StringType {
    Disco { disco: Disco },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IndicatorStrings {
    pub horizontal_spacing: f64,
    pub vertical_spacing: f64,
    pub radius_x: f64,
    pub radius_y: f64,
    #[serde(flatten)]
    pub strings: StringType,
}

impl Default for IndicatorStrings {
    fn default() -> Self {
        Self {
            horizontal_spacing: 10.0,
            vertical_spacing: 5.0,
            radius_x: 2.0,
            radius_y: 2.0,
            strings: StringType::Disco { disco: Disco::default() },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IndicatorType {
    Strings { strings: IndicatorStrings },
    Circle { circle: IndicatorCircle },
    Classic { classic: IndicatorClassic },
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
            indicator_type: IndicatorType::Strings {
                strings: IndicatorStrings::default(),
            },
            common: IndicatorCommon::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IndicatorCommon {
    pub border_width: f64,
    pub blink: bool,
    pub foreground: Rgba,
    pub background: Rgba,
    #[serde(deserialize_with = "option_explicit_none")]
    pub background_stop: Option<Rgba>,
    pub border_color: Rgba,
    pub border_color_focused: Rgba,
    pub indicator_color: Rgba,
    #[serde(deserialize_with = "option_explicit_none")]
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
