//! One-key view and UCS shortcuts, ported from the user's lisp:
//! `01`-`09` view presets, `ZE` zoom extents, `UV` restore the UCS saved by DVT, `UW` UCS
//! World and view twist 0.

/// The host command line for a shortcut, or `None` if `cmd` is not one of them.
pub fn host_command(cmd: &str) -> Option<&'static str> {
    Some(match cmd.to_ascii_uppercase().as_str() {
        "01" => "-VIEW SWISO",
        "02" => "-VIEW FRONT",
        "03" => "-VIEW SEISO",
        "04" => "-VIEW LEFT",
        "05" => "-VIEW TOP",
        "06" => "-VIEW RIGHT",
        "07" => "-VIEW NWISO",
        "08" => "-VIEW BACK",
        "09" => "-VIEW NEISO",
        "ZE" => "ZOOM EXTENTS",
        // Two host commands, one per line: `lines()` them and run each separately.
        "UW" => "UCS WORLD\nDVIEW TW 0",
        // The host has no UCS RESTORE; `UCS <name>` activates a saved UCS.
        "UV" => "UCS CURRENT_VIEW",
        _ => return None,
    })
}

#[cfg(test)]
const COMMANDS: [&str; 12] = ["01", "02", "03", "04", "05", "06", "07", "08", "09", "ZE", "UV", "UW"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_shortcuts_map_as_in_the_lisp() {
        assert_eq!(host_command("01"), Some("-VIEW SWISO"));
        assert_eq!(host_command("03"), Some("-VIEW SEISO"));
        assert_eq!(host_command("05"), Some("-VIEW TOP"));
        assert_eq!(host_command("09"), Some("-VIEW NEISO"));
        assert_eq!(host_command("ze"), Some("ZOOM EXTENTS"));
        assert_eq!(host_command("UV"), Some("UCS CURRENT_VIEW"));
        assert_eq!(host_command("uw").unwrap().lines().collect::<Vec<_>>(), ["UCS WORLD", "DVIEW TW 0"]);
        assert_eq!(host_command("10"), None);
        assert_eq!(host_command("STEAL"), None);
        assert!(COMMANDS.iter().all(|c| host_command(c).is_some()));
    }
}
