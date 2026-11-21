//! The panel's ID: the short name given to it at installation (`docs/install.md`), one line in
//! `/mnt/persist/nspanel/panel_name`. It names the panel everywhere: its devices in Home Assistant, its ESPHome device
//! and mDNS name, its Sendspin player, and `panel_id` towards the PanelOS integration. A panel without a stored name
//! is `nspanel-pro`, as every panel was before names, so its devices and entities stay as they are.
//!
//! Read once, at the start: a new name takes effect when the app starts again.

use std::sync::OnceLock;

/// The ID of a panel without a name of its own.
pub const DEFAULT: &str = "nspanel-pro";
/// The name, one line (written by the install guide; changed the same way).
pub const FILE: &str = "/mnt/persist/nspanel/panel_name";
/// As long as an ESPHome device name may be.
pub const MAX_LEN: usize = 31;

/// The name in the file's text (its first line): None for an empty file, upper-case letters taken as lower case. An
/// error for a name outside the rules: letters a-z, digits and hyphens, at most 31 characters, no hyphen at either end
/// (the name is also the panel's mDNS host name).
pub fn parse(text: &str) -> Result<Option<String>, String> {
    let name = text.lines().next().unwrap_or("").trim().to_ascii_lowercase();
    if name.is_empty() {
        return Ok(None);
    }
    if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return Err("only letters a-z, digits and hyphens may be in it".into());
    }
    if name.len() > MAX_LEN {
        return Err(format!("it is longer than {MAX_LEN} characters"));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err("it starts or ends with a hyphen".into());
    }
    Ok(Some(name))
}

/// The ID for the file's text (None: no file) and, for a name that is not used, why.
fn resolve(text: Option<&str>) -> (String, Option<String>) {
    match text.map(parse) {
        None | Some(Ok(None)) => (DEFAULT.to_string(), None),
        Some(Ok(Some(name))) => (name, None),
        Some(Err(why)) => (DEFAULT.to_string(), Some(why)),
    }
}

static ID: OnceLock<String> = OnceLock::new();

/// The panel's ID: its name, or `nspanel-pro` without one.
pub fn get() -> &'static str {
    ID.get_or_init(|| {
        let text = std::fs::read_to_string(FILE).ok();
        let (id, refused) = resolve(text.as_deref());
        match refused {
            Some(why) => eprintln!("nspanel-app: the name in {FILE} is not used ({why}): the panel is {DEFAULT}"),
            None if id != DEFAULT => eprintln!("nspanel-app: the panel's name is {id}"),
            None => {}
        }
        id
    })
}

/// A name shown in Home Assistant and Music Assistant: `base` alone for a panel without a name of its own, else with
/// the name, e.g. "NSPanel Pro (kitchen)".
pub fn label(base: &str) -> String {
    label_for(base, get())
}

fn label_for(base: &str, id: &str) -> String {
    if id == DEFAULT {
        base.to_string()
    } else {
        format!("{base} ({id})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_by_the_rules() {
        assert_eq!(parse("kitchen"), Ok(Some("kitchen".into())));
        assert_eq!(parse("kitchen\n"), Ok(Some("kitchen".into())));
        assert_eq!(parse("  Kitchen2 \r\n"), Ok(Some("kitchen2".into())));
        assert_eq!(parse("living-room"), Ok(Some("living-room".into())));
        assert_eq!(parse("2nd"), Ok(Some("2nd".into())));
        assert_eq!(parse("a"), Ok(Some("a".into())));
        assert_eq!(parse(&"a".repeat(MAX_LEN)), Ok(Some("a".repeat(MAX_LEN))));
        // only the first line counts
        assert_eq!(parse("hall\nkitchen\n"), Ok(Some("hall".into())));
        // the old fixed ID written as a name is the same panel
        assert_eq!(parse("nspanel-pro"), Ok(Some(DEFAULT.into())));
    }

    #[test]
    fn no_name_when_empty() {
        assert_eq!(parse(""), Ok(None));
        assert_eq!(parse("\n"), Ok(None));
        assert_eq!(parse("   \n"), Ok(None));
    }

    #[test]
    fn names_outside_the_rules() {
        for bad in ["living room", "kitchen_1", "küche", "kitchen.local", "-kitchen", "kitchen-", "-", "a/b", "\"x\""] {
            assert!(parse(bad).is_err(), "{bad:?} should be refused");
        }
        assert!(parse(&"a".repeat(MAX_LEN + 1)).is_err());
    }

    #[test]
    fn the_default_without_a_usable_name() {
        assert_eq!(resolve(None), (DEFAULT.to_string(), None));
        assert_eq!(resolve(Some("")), (DEFAULT.to_string(), None));
        assert_eq!(resolve(Some("kitchen\n")), ("kitchen".to_string(), None));
        let (id, why) = resolve(Some("living room"));
        assert_eq!(id, DEFAULT);
        assert!(why.is_some());
    }

    #[test]
    fn labels_carry_the_name() {
        assert_eq!(label_for("NSPanel Pro", DEFAULT), "NSPanel Pro");
        assert_eq!(label_for("NSPanel Pro Voice", DEFAULT), "NSPanel Pro Voice");
        assert_eq!(label_for("NSPanel Pro", "kitchen"), "NSPanel Pro (kitchen)");
        assert_eq!(label_for("NSPanel Pro Voice", "kitchen"), "NSPanel Pro Voice (kitchen)");
    }
}
