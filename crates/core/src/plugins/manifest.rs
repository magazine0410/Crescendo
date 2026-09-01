use anyhow::{anyhow, bail, Result};
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

const MAX_TEXT_LEN: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginKind {
    Declarative,
    Wasm,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub authors: Vec<String>,
    pub license: String,
    #[serde(default)]
    pub homepage: Option<String>,
    pub kind: PluginKind,
    pub api_version: String,
    pub min_crescendo_version: String,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub contributes: PluginContributions,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PluginContributions {
    pub settings: Vec<PluginSettingDeclaration>,
    pub facet_transforms: Vec<PluginFacetTransform>,
    pub menu_controls: Vec<PluginMenuControl>,
}

/// A deliberately small vocabulary compiled by Crescendo into trusted query
/// plans. Descriptors carry data only; they never contain SQL or code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PluginFacetTransform {
    #[serde(rename = "text.first_segment")]
    TextFirstSegment {
        target_tag: String,
        delimiter: String,
        trim_whitespace: bool,
        enabled_setting: String,
    },
}

impl PluginFacetTransform {
    pub fn target_tag(&self) -> &str {
        match self {
            Self::TextFirstSegment { target_tag, .. } => target_tag,
        }
    }

    pub fn enabled_setting(&self) -> &str {
        match self {
            Self::TextFirstSegment {
                enabled_setting, ..
            } => enabled_setting,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginMenuSlot {
    #[serde(rename = "library.filter")]
    LibraryFilter,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginMenuVisibility {
    pub active_tag: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PluginMenuAction {
    ToggleSetting { setting: String },
}

impl PluginMenuAction {
    pub fn setting(&self) -> &str {
        match self {
            Self::ToggleSetting { setting } => setting,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginMenuControl {
    pub id: String,
    pub slot: PluginMenuSlot,
    pub label: String,
    pub visible_when: PluginMenuVisibility,
    pub checked_setting: String,
    pub action: PluginMenuAction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PluginSettingDeclaration {
    Boolean {
        key: String,
        label: String,
        #[serde(default)]
        description: String,
        default: bool,
    },
    String {
        key: String,
        label: String,
        #[serde(default)]
        description: String,
        default: String,
        #[serde(default)]
        min_length: Option<usize>,
        #[serde(default)]
        max_length: Option<usize>,
    },
    Number {
        key: String,
        label: String,
        #[serde(default)]
        description: String,
        default: f64,
        #[serde(default)]
        min: Option<f64>,
        #[serde(default)]
        max: Option<f64>,
        #[serde(default)]
        step: Option<f64>,
    },
    Select {
        key: String,
        label: String,
        #[serde(default)]
        description: String,
        default: String,
        options: Vec<PluginSelectOption>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginSelectOption {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PluginSettingValue {
    #[serde(flatten)]
    pub declaration: PluginSettingDeclaration,
    pub value: Value,
    pub affects_projection: bool,
}

impl PluginManifest {
    pub fn validate(&self, has_module: bool) -> Result<()> {
        if self.schema_version != 1 {
            bail!("unsupported plugin manifest schema {}", self.schema_version);
        }
        validate_plugin_id(&self.id)?;
        validate_text("name", &self.name, false)?;
        validate_text("description", &self.description, true)?;
        validate_text("license", &self.license, false)?;
        if self.authors.is_empty() {
            bail!("authors must contain at least one name");
        }
        if self.authors.len() > 64 {
            bail!("authors cannot contain more than 64 names");
        }
        for author in &self.authors {
            validate_text("author", author, false)?;
        }
        if let Some(homepage) = &self.homepage {
            validate_text("homepage", homepage, false)?;
            if !homepage.starts_with("https://") && !homepage.starts_with("http://") {
                bail!("homepage must use http or https");
            }
        }
        Version::parse(&self.version)
            .map_err(|_| anyhow!("version must be semantic versioning"))?;
        Version::parse(&self.api_version)
            .map_err(|_| anyhow!("api_version must be semantic versioning"))?;
        Version::parse(&self.min_crescendo_version)
            .map_err(|_| anyhow!("min_crescendo_version must be semantic versioning"))?;

        if self.kind == PluginKind::Declarative {
            if has_module {
                bail!("declarative plugins must not contain module.wasm");
            }
            if !self.permissions.is_empty() {
                bail!("declarative plugins cannot request permissions");
            }
        } else if !has_module {
            bail!("Wasm plugins must contain module.wasm");
        }

        if self.permissions.len() > 128 {
            bail!("permissions cannot contain more than 128 entries");
        }
        if self.contributes.settings.len() > 128 {
            bail!("plugins cannot declare more than 128 settings");
        }
        if self.contributes.facet_transforms.len() > 32 {
            bail!("plugins cannot declare more than 32 facet transforms");
        }
        if self.contributes.menu_controls.len() > 64 {
            bail!("plugins cannot declare more than 64 menu controls");
        }
        let mut settings = HashSet::new();
        for setting in &self.contributes.settings {
            setting.validate_declaration()?;
            if !settings.insert(setting.key()) {
                bail!("duplicate setting key {:?}", setting.key());
            }
        }
        let boolean_settings: HashSet<&str> = self
            .contributes
            .settings
            .iter()
            .filter_map(|setting| match setting {
                PluginSettingDeclaration::Boolean { key, .. } => Some(key.as_str()),
                _ => None,
            })
            .collect();
        let mut transform_targets = HashSet::new();
        for transform in &self.contributes.facet_transforms {
            match transform {
                PluginFacetTransform::TextFirstSegment {
                    target_tag,
                    delimiter,
                    enabled_setting,
                    ..
                } => {
                    validate_target_tag(target_tag)?;
                    if delimiter.is_empty()
                        || delimiter.len() > 32
                        || delimiter.chars().any(char::is_control)
                    {
                        bail!("facet-transform delimiters must contain 1 to 32 printable bytes");
                    }
                    if !boolean_settings.contains(enabled_setting.as_str()) {
                        bail!(
                            "facet transform setting {enabled_setting:?} must name a declared boolean setting"
                        );
                    }
                    if !transform_targets.insert(target_tag.as_str()) {
                        bail!("a plugin cannot transform target tag {target_tag:?} more than once");
                    }
                }
            }
        }
        let mut menu_ids = HashSet::new();
        for control in &self.contributes.menu_controls {
            validate_setting_key(&control.id)?;
            validate_text("menu label", &control.label, false)?;
            validate_target_tag(&control.visible_when.active_tag)?;
            if !menu_ids.insert(control.id.as_str()) {
                bail!("duplicate menu control id {:?}", control.id);
            }
            if !boolean_settings.contains(control.checked_setting.as_str()) {
                bail!(
                    "menu checked_setting {:?} must name a declared boolean setting",
                    control.checked_setting
                );
            }
            if control.action.setting() != control.checked_setting {
                bail!("a toggle_setting menu action must toggle its checked_setting");
            }
        }
        Ok(())
    }
}

impl PluginSettingDeclaration {
    pub fn key(&self) -> &str {
        match self {
            Self::Boolean { key, .. }
            | Self::String { key, .. }
            | Self::Number { key, .. }
            | Self::Select { key, .. } => key,
        }
    }

    pub fn default_value(&self) -> Value {
        match self {
            Self::Boolean { default, .. } => Value::Bool(*default),
            Self::String { default, .. } | Self::Select { default, .. } => {
                Value::String(default.clone())
            }
            Self::Number { default, .. } => serde_json::Number::from_f64(*default)
                .map(Value::Number)
                .expect("validated finite setting default"),
        }
    }

    pub fn validate_value(&self, value: &Value) -> Result<()> {
        match self {
            Self::Boolean { .. } if value.is_boolean() => Ok(()),
            Self::String {
                min_length,
                max_length,
                ..
            } => {
                let value = value
                    .as_str()
                    .ok_or_else(|| anyhow!("value must be a string"))?;
                let len = value.chars().count();
                let minimum = min_length.unwrap_or(0);
                let maximum = max_length.unwrap_or(MAX_TEXT_LEN);
                if len < minimum || len > maximum {
                    bail!("string length must be between {minimum} and {maximum}");
                }
                Ok(())
            }
            Self::Number { min, max, step, .. } => {
                let number = value
                    .as_f64()
                    .filter(|number| number.is_finite())
                    .ok_or_else(|| anyhow!("value must be a finite number"))?;
                if min.is_some_and(|minimum| number < minimum)
                    || max.is_some_and(|maximum| number > maximum)
                {
                    bail!("number is outside the declared range");
                }
                if let (Some(step), Some(origin)) = (step, min.or(Some(0.0))) {
                    let steps = (number - origin) / step;
                    if (steps - steps.round()).abs() > 1e-9 {
                        bail!("number does not align with the declared step");
                    }
                }
                Ok(())
            }
            Self::Select { options, .. } => {
                let value = value
                    .as_str()
                    .ok_or_else(|| anyhow!("value must be a string"))?;
                if !options.iter().any(|option| option.value == value) {
                    bail!("value is not one of the declared choices");
                }
                Ok(())
            }
            _ => bail!("value has the wrong type"),
        }
    }

    fn validate_declaration(&self) -> Result<()> {
        validate_setting_key(self.key())?;
        let (label, description) = match self {
            Self::Boolean {
                label, description, ..
            }
            | Self::String {
                label, description, ..
            }
            | Self::Number {
                label, description, ..
            }
            | Self::Select {
                label, description, ..
            } => (label, description),
        };
        validate_text("setting label", label, false)?;
        validate_text("setting description", description, true)?;

        match self {
            Self::String {
                min_length,
                max_length,
                ..
            } => {
                if max_length.is_some_and(|maximum| maximum > MAX_TEXT_LEN) {
                    bail!("setting max_length cannot exceed {MAX_TEXT_LEN}");
                }
                if min_length.unwrap_or(0) > max_length.unwrap_or(MAX_TEXT_LEN) {
                    bail!("setting min_length cannot exceed max_length");
                }
            }
            Self::Number {
                default,
                min,
                max,
                step,
                ..
            } => {
                if !default.is_finite()
                    || min.is_some_and(|value| !value.is_finite())
                    || max.is_some_and(|value| !value.is_finite())
                {
                    bail!("number setting values must be finite");
                }
                if min
                    .zip(*max)
                    .is_some_and(|(minimum, maximum)| minimum > maximum)
                {
                    bail!("number setting min cannot exceed max");
                }
                if step.is_some_and(|value| !value.is_finite() || value <= 0.0) {
                    bail!("number setting step must be positive and finite");
                }
            }
            Self::Select {
                default, options, ..
            } => {
                if options.is_empty() || options.len() > 128 {
                    bail!("select settings must have between 1 and 128 choices");
                }
                let mut values = HashSet::new();
                for option in options {
                    validate_text("select value", &option.value, false)?;
                    validate_text("select label", &option.label, false)?;
                    if !values.insert(option.value.as_str()) {
                        bail!("select setting contains a duplicate choice");
                    }
                }
                if !values.contains(default.as_str()) {
                    bail!("select setting default is not one of its choices");
                }
            }
            Self::Boolean { .. } => {}
        }
        self.validate_value(&self.default_value())
    }
}

fn validate_plugin_id(id: &str) -> Result<()> {
    if id.len() > 128 || id.split('.').count() < 3 {
        bail!("plugin id must be a lowercase reverse-domain identifier");
    }
    for segment in id.split('.') {
        if segment.is_empty()
            || segment.len() > 63
            || !segment.as_bytes()[0].is_ascii_lowercase()
            || segment.ends_with('-')
            || !segment
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            bail!("plugin id must be a lowercase reverse-domain identifier");
        }
    }
    Ok(())
}

fn validate_setting_key(key: &str) -> Result<()> {
    if key.is_empty()
        || key.len() > 64
        || !key.as_bytes()[0].is_ascii_lowercase()
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        bail!("setting keys must use lowercase letters, digits, and underscores");
    }
    Ok(())
}

fn validate_target_tag(tag: &str) -> Result<()> {
    if tag.is_empty()
        || tag.len() > 64
        || tag != tag.trim().to_ascii_lowercase()
        || crate::search::browse_column(tag).is_none()
    {
        bail!("contribution target tags must be canonical browsable tag names");
    }
    Ok(())
}

fn validate_text(field: &str, value: &str, allow_empty: bool) -> Result<()> {
    if (!allow_empty && value.trim().is_empty()) || value.len() > MAX_TEXT_LEN {
        bail!("{field} is empty or too long");
    }
    if value.chars().any(char::is_control) {
        bail!("{field} contains control characters");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> PluginManifest {
        PluginManifest {
            schema_version: 1,
            id: "dev.crescendo.example".into(),
            name: "Example".into(),
            version: "1.2.3".into(),
            description: "Example plugin".into(),
            authors: vec!["Crescendo".into()],
            license: "MIT".into(),
            homepage: Some("https://example.invalid".into()),
            kind: PluginKind::Declarative,
            api_version: "1.0.0".into(),
            min_crescendo_version: "0.8.0".into(),
            permissions: vec![],
            contributes: PluginContributions::default(),
        }
    }

    #[test]
    fn manifest_rejects_unknown_fields() {
        let json = serde_json::to_value(manifest()).unwrap();
        let mut object = json.as_object().unwrap().clone();
        object.insert("surprise".into(), Value::Bool(true));
        assert!(serde_json::from_value::<PluginManifest>(Value::Object(object)).is_err());
    }

    #[test]
    fn manifest_rejects_invalid_schema_identity_and_versions() {
        let mut value = manifest();
        value.schema_version = 2;
        assert!(value.validate(false).is_err());

        value = manifest();
        value.id = "Crescendo Plugin".into();
        assert!(value.validate(false).is_err());

        value = manifest();
        value.version = "tomorrow".into();
        assert!(value.validate(false).is_err());

        value = manifest();
        value.api_version = "v1".into();
        assert!(value.validate(false).is_err());

        value = manifest();
        value.min_crescendo_version = "latest".into();
        assert!(value.validate(false).is_err());
    }

    #[test]
    fn declarative_manifest_rejects_code_and_permissions() {
        let mut value = manifest();
        assert!(value.validate(true).is_err());
        value.permissions.push("library.read".into());
        assert!(value.validate(false).is_err());
    }

    #[test]
    fn setting_values_follow_the_declared_constraints() {
        let setting = PluginSettingDeclaration::Number {
            key: "amount".into(),
            label: "Amount".into(),
            description: String::new(),
            default: 2.0,
            min: Some(0.0),
            max: Some(4.0),
            step: Some(0.5),
        };
        setting.validate_declaration().unwrap();
        setting.validate_value(&serde_json::json!(3.5)).unwrap();
        assert!(setting.validate_value(&serde_json::json!(3.2)).is_err());
        assert!(setting.validate_value(&serde_json::json!(9)).is_err());
    }

    #[test]
    fn setting_values_serialize_as_a_flat_discriminated_object() {
        let value = PluginSettingValue {
            declaration: PluginSettingDeclaration::Boolean {
                key: "enabled".into(),
                label: "Enabled".into(),
                description: String::new(),
                default: false,
            },
            value: Value::Bool(true),
            affects_projection: false,
        };
        assert_eq!(
            serde_json::to_value(value).unwrap(),
            serde_json::json!({
                "type": "boolean",
                "key": "enabled",
                "label": "Enabled",
                "description": "",
                "default": false,
                "value": true,
                "affects_projection": false
            })
        );
    }

    #[test]
    fn facet_and_menu_contributions_are_bounded_and_reference_boolean_settings() {
        let mut value = manifest();
        value.contributes.settings.push(PluginSettingDeclaration::Boolean {
            key: "collapse_titles".into(),
            label: "Collapse titles".into(),
            description: String::new(),
            default: true,
        });
        value
            .contributes
            .facet_transforms
            .push(PluginFacetTransform::TextFirstSegment {
                target_tag: "grouping".into(),
                delimiter: ";".into(),
                trim_whitespace: true,
                enabled_setting: "collapse_titles".into(),
            });
        value.contributes.menu_controls.push(PluginMenuControl {
            id: "collapse_titles".into(),
            slot: PluginMenuSlot::LibraryFilter,
            label: "Collapse multi-tagged titles under their first grouping.".into(),
            visible_when: PluginMenuVisibility {
                active_tag: "grouping".into(),
            },
            checked_setting: "collapse_titles".into(),
            action: PluginMenuAction::ToggleSetting {
                setting: "collapse_titles".into(),
            },
        });
        value.validate(false).unwrap();

        let mut bad = value.clone();
        bad.contributes.facet_transforms[0] = PluginFacetTransform::TextFirstSegment {
            target_tag: "not_a_real_tag".into(),
            delimiter: ";".into(),
            trim_whitespace: true,
            enabled_setting: "collapse_titles".into(),
        };
        assert!(bad.validate(false).is_err());

        let mut bad = value;
        bad.contributes.menu_controls[0].checked_setting = "missing".into();
        assert!(bad.validate(false).is_err());
    }
}
