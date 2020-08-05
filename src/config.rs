use core::convert::TryFrom;
/// This module defines configuration items that we support
use serde::{Deserialize, Serialize};

trait Validate {
    type Arg: ?Sized;
    /// returns true when the config value is valid and hence can be set
    fn is_valid(&self, _v: &Self::Arg) -> bool {
        true
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ConfigItem {
    pub name: String,
    pub title: String,
    #[serde(flatten)]
    pub value: ConfigValue,
}

/// List of settings that we support
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")] // this is slower but more usual
#[serde(rename_all = "lowercase")]
pub enum ConfigValue {
    String(ConfigString),
    Integer(ConfigInteger),
    Selection(ConfigSelection),
    Bool(ConfigBool),
}

impl ConfigValue {
    pub fn try_set_value(&mut self, s: &str) -> bool {
        match self {
            ConfigValue::String(conf) => {
                if conf.is_valid(s) {
                    conf.value = s.to_owned();
                    true
                } else {
                    false
                }
            }
            ConfigValue::Integer(conf) => match s.parse::<u32>() {
                Ok(i) => {
                    if conf.is_valid(&i) {
                        conf.0.value = i;
                        true
                    } else {
                        false
                    }
                }
                Err(_) => false,
            },
            ConfigValue::Selection(conf) => {
                if conf.is_valid(s) {
                    conf.0.value = s.to_owned();
                    true
                } else {
                    false
                }
            }
            ConfigValue::Bool(conf) => {
                match s {
                    "on" => {
                        conf.value = true;
                    }
                    _ => {
                        conf.value = false;
                    }
                }
                true
            }
        }
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigString {
    pub value: String,
}

impl Validate for ConfigString {
    type Arg = str;
}

impl From<&str> for ConfigString {
    fn from(s: &str) -> Self {
        Self {
            value: s.to_owned(),
        }
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigBool {
    value: bool,
}

impl ConfigBool {
    pub fn new(value: bool) -> Self {
        ConfigBool { value }
    }
}

impl Validate for ConfigBool {
    type Arg = bool;
}

impl From<bool> for ConfigBool {
    fn from(b: bool) -> Self {
        Self { value: b }
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RawConfigInteger {
    min: u32,
    max: u32,
    value: u32,
}

macro_rules! validated {
    (
        $( #[$attr:meta] )*
        $vis:vis $type:ident ($parent:ty)
    ) => {
        $( #[$attr] )*
        #[derive(Serialize)]
        $vis struct $type($parent);

        impl<'de> serde::Deserialize<'de> for $type
        {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                use core::convert::TryInto;
                use serde::de::Error;
                <$parent as serde::Deserialize>::deserialize(deserializer)?
                    .try_into()
                    .map_err(D::Error::custom)
            }
        }

        impl core::ops::Deref for $type {
            type Target = $parent;
            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }
    };
}

impl Validate for ConfigInteger {
    type Arg = u32;
    fn is_valid(&self, v: &Self::Arg) -> bool {
        self.min <= *v && *v <= self.max
    }
}

impl TryFrom<RawConfigInteger> for ConfigInteger {
    type Error = &'static str;

    fn try_from(raw: RawConfigInteger) -> Result<Self, Self::Error> {
        if raw.min <= raw.value && raw.value <= raw.max {
            Ok(Self(raw))
        } else {
            Err("value is not in range")
        }
    }
}

validated! {#[derive(Clone, PartialEq)] pub ConfigInteger(RawConfigInteger)}

impl ConfigInteger {
    pub fn new(min: u32, max: u32, value: u32) -> Result<Self, &'static str> {
        Self::try_from(RawConfigInteger { min, max, value })
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RawConfigSelection {
    value: String,
    options: Vec<Choice>,
}

validated! {#[derive(Clone, PartialEq)] pub ConfigSelection(RawConfigSelection)}

impl ConfigSelection {
    pub fn new(value: String, options: Vec<Choice>) -> Result<Self, &'static str> {
        Self::try_from(RawConfigSelection { value, options })
    }
}

impl Validate for ConfigSelection {
    type Arg = str;
    fn is_valid(&self, v: &Self::Arg) -> bool {
        self.options.iter().find(|&elem| elem.value == v).is_some()
    }
}

impl TryFrom<RawConfigSelection> for ConfigSelection {
    type Error = &'static str;

    fn try_from(raw: RawConfigSelection) -> Result<Self, Self::Error> {
        if raw
            .options
            .iter()
            .find(|&elem| elem.value == raw.value)
            .is_some()
        {
            Ok(Self(raw))
        } else {
            Err("value does not match any choices")
        }
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    value: String,
    title: String,
}

impl Choice {
    pub fn new(value: String, title: String) -> Self {
        Self { value, title }
    }
}
