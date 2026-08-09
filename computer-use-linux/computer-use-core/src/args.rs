//! Strict tool-argument reader, mirroring `ToolCatalog.swift`'s
//! `ArgumentReader`: every field must be consumed and unknown fields fail
//! closed.

use serde_json::Value;

use crate::models::{ComputerUseError, JsonObject, Result};

pub struct ArgumentReader {
    remaining: JsonObject,
}

impl ArgumentReader {
    pub fn new(arguments: JsonObject) -> Self {
        Self {
            remaining: arguments,
        }
    }

    pub fn required_string(&mut self, key: &str, allow_empty: bool) -> Result<String> {
        match self.remaining.remove(key) {
            Some(Value::String(value)) if allow_empty || !value.is_empty() => Ok(value),
            _ => Err(ComputerUseError::InvalidArguments(format!(
                "{key} must be a string{}.",
                if allow_empty {
                    ""
                } else {
                    " with at least one character"
                }
            ))),
        }
    }

    pub fn optional_string(&mut self, key: &str) -> Result<Option<String>> {
        match self.remaining.remove(key) {
            None => Ok(None),
            Some(Value::String(value)) if !value.is_empty() => Ok(Some(value)),
            Some(_) => Err(ComputerUseError::InvalidArguments(format!(
                "{key} must be a non-empty string."
            ))),
        }
    }

    pub fn required_number(&mut self, key: &str) -> Result<f64> {
        self.remaining
            .remove(key)
            .as_ref()
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite())
            .ok_or_else(|| {
                ComputerUseError::InvalidArguments(format!("{key} must be a finite number."))
            })
    }

    pub fn optional_number(&mut self, key: &str, default: f64) -> Result<f64> {
        match self.remaining.remove(key) {
            None => Ok(default),
            Some(raw) => raw
                .as_f64()
                .filter(|value| value.is_finite())
                .ok_or_else(|| {
                    ComputerUseError::InvalidArguments(format!("{key} must be a finite number."))
                }),
        }
    }

    pub fn optional_integer(&mut self, key: &str) -> Result<Option<i64>> {
        match self.remaining.remove(key) {
            None => Ok(None),
            Some(raw) => raw.as_i64().map(Some).ok_or_else(|| {
                ComputerUseError::InvalidArguments(format!("{key} must be an integer."))
            }),
        }
    }

    pub fn optional_integer_default(&mut self, key: &str, default: i64) -> Result<i64> {
        Ok(self.optional_integer(key)?.unwrap_or(default))
    }

    pub fn required_object(&mut self, key: &str) -> Result<JsonObject> {
        match self.remaining.remove(key) {
            Some(Value::Object(value)) => Ok(value),
            _ => Err(ComputerUseError::InvalidArguments(format!(
                "{key} must be an object."
            ))),
        }
    }

    pub fn optional_string_array(&mut self, key: &str) -> Result<Vec<String>> {
        match self.remaining.remove(key) {
            None => Ok(Vec::new()),
            Some(Value::Array(values)) => values
                .into_iter()
                .map(|value| match value {
                    Value::String(string) => Ok(string),
                    _ => Err(ComputerUseError::InvalidArguments(format!(
                        "{key} must contain only strings."
                    ))),
                })
                .collect(),
            Some(_) => Err(ComputerUseError::InvalidArguments(format!(
                "{key} must be an array."
            ))),
        }
    }

    pub fn finish(self) -> Result<()> {
        if self.remaining.is_empty() {
            return Ok(());
        }
        let mut keys: Vec<&str> = self.remaining.keys().map(String::as_str).collect();
        keys.sort_unstable();
        Err(ComputerUseError::InvalidArguments(format!(
            "Unknown argument fields: {}.",
            keys.join(", ")
        )))
    }
}
