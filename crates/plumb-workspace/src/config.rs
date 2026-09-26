//! Workspace policy configuration, separate from cached document semantics.
use serde::Deserialize;
use std::{io::ErrorKind, path::Path};

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkspaceConfig {
    pub check: CheckSettings,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct CheckSettings {
    pub event_category: CheckRuleSettings,
    pub event_timeline: CheckRuleSettings,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CheckRuleSettings {
    pub enabled: bool,
}

impl WorkspaceConfig {
    /// Missing files use defaults; malformed files cannot be masked by overrides.
    pub fn load(root: &Path, overrides: &[String]) -> Result<Self, String> {
        let path = root.join(".plumb/config.toml");
        let source = match std::fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) if error.kind() == ErrorKind::NotFound => String::new(),
            Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
        };
        Self::parse(&source, overrides).map_err(|error| format!("{}: {error}", path.display()))
    }

    pub fn parse(source: &str, overrides: &[String]) -> Result<Self, String> {
        let mut config: Self = toml::from_str(source).map_err(|error| error.to_string())?;
        for assignment in overrides {
            let (key, value) = assignment
                .split_once('=')
                .ok_or_else(|| format!("invalid --config {assignment:?}: expected KEY=VALUE"))?;
            let key = key.trim();
            let target = match key {
                "check.event-category.enabled" => &mut config.check.event_category.enabled,
                "check.event-timeline.enabled" => &mut config.check.event_timeline.enabled,
                _ => return Err(format!("unknown configuration key {key:?}")),
            };
            // Deserialize exactly one TOML value, never a document assembled from user text.
            let value: toml::Value = value
                .trim()
                .parse()
                .map_err(|error| format!("invalid value for {key}: {error}"))?;
            *target = value
                .as_bool()
                .ok_or_else(|| format!("{key} must be a boolean"))?;
        }
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_ordered_overrides_preserve_other_rules() {
        let config = WorkspaceConfig::parse("", &[]).unwrap();
        assert!(!config.check.event_category.enabled && !config.check.event_timeline.enabled);
        let config = WorkspaceConfig::parse(
            "[check.event-category]\nenabled = true\n",
            &[
                "check.event-category.enabled=false".into(),
                "check.event-timeline.enabled=true".into(),
                "check.event-category.enabled=true".into(),
            ],
        )
        .unwrap();
        assert!(config.check.event_category.enabled && config.check.event_timeline.enabled);
    }

    #[test]
    fn rejects_unknown_keys_types_and_additional_declarations() {
        for source in [
            "unknown = true",
            "[check.unknown]",
            "[check.event-category]\nexplicit = true",
            "[check.event-timeline]\nenabled = 'true'",
            "[check.event-timeline]\nfrom = 'now'",
        ] {
            assert!(
                WorkspaceConfig::parse(source, &["check.event-timeline.enabled=true".into()])
                    .is_err(),
                "{source}"
            );
        }
        for value in [
            "'true'",
            "1",
            "[]",
            "{}",
            "true\nother = false",
            "true\n[check]",
            "",
        ] {
            assert!(
                WorkspaceConfig::parse("", &[format!("check.event-timeline.enabled={value}")])
                    .is_err(),
                "{value}"
            );
        }
        for assignment in [
            "check.event-category",
            "check.event-category.explicit=true",
            "check.event-timeline.from='now'",
        ] {
            assert!(WorkspaceConfig::parse("", &[assignment.into()]).is_err());
        }
    }

    #[test]
    fn loads_only_root_config_and_does_not_create_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            !WorkspaceConfig::load(dir.path(), &[])
                .unwrap()
                .check
                .event_category
                .enabled
        );
        assert!(!dir.path().join(".plumb").exists());
        std::fs::create_dir(dir.path().join(".plumb")).unwrap();
        std::fs::write(
            dir.path().join(".plumb/config.toml"),
            "[check.event-category]\nenabled=true",
        )
        .unwrap();
        assert!(
            WorkspaceConfig::load(dir.path(), &[])
                .unwrap()
                .check
                .event_category
                .enabled
        );
        let child = dir.path().join("child");
        std::fs::create_dir(&child).unwrap();
        assert!(
            !WorkspaceConfig::load(&child, &[])
                .unwrap()
                .check
                .event_category
                .enabled
        );
        std::fs::remove_file(dir.path().join(".plumb/config.toml")).unwrap();
        std::fs::create_dir(dir.path().join(".plumb/config.toml")).unwrap();
        assert!(WorkspaceConfig::load(dir.path(), &[]).is_err());
    }
}
