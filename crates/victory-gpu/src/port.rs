//! Renderer ports scaffolding (`VICTORY_PORT_*`, all off by default).
//!
//! Port 1 (`VICTORY_PORT_CONSTANTS`) is an alias of the existing
//! [`crate::inplace::constants_on`] path — the title's 32-bit graphics /
//! constants heap read in place. No title patch bytes live here; they belong
//! in `victory-game` if a later port needs them.
//!
//! Follow-ups (not in this PR):
//! - `VICTORY_PORT_CBV` / `VICTORY_PORT_CBV_COPIES` (AvatarManz #363)
//! - `VICTORY_PORT_DESCRIPTORS` (AvatarManz #384 / #359)
//! - `VICTORY_PORT_GUARD`, `VICTORY_PORT_JOURNAL`

#[cfg(test)]
mod tests {
    #[test]
    fn port_constants_is_off_unless_env_asks() {
        // Read once from the environment in a fresh process; without the
        // switch the alias must leave the old path alone.
        if std::env::var_os("VICTORY_PORT_CONSTANTS").is_none()
            && std::env::var_os("VICTORY_INPLACE_CONSTANTS").is_none()
        {
            assert!(!crate::inplace::constants_on());
        }
    }
}
