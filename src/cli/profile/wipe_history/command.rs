use super::common::parse_older_than;
use crate::profile::wipe_history;
use anyhow::Result;

/// Delete non-current versions of the profile
pub fn cmd_wipe_history(older_than: Option<&str>, dry_run: bool) -> Result<()> {
    let older_than_duration = if let Some(ot) = older_than {
        Some(std::time::Duration::from_secs(parse_older_than(ot)?))
    } else {
        None
    };

    wipe_history(older_than_duration, dry_run)
}
