//! Optional window lookup for choosing between running instances.

use std::{env, process::Command};

fn location() -> Option<(u32, String)> {
    // TMUX ends with the server PID and session number. Pane IDs are
    // unique on that server, even when a pane moves to another window.
    let tmux = env::var("TMUX").ok()?;
    let server = tmux.rsplit(',').nth(1)?.parse().ok()?;
    let pane = env::var("TMUX_PANE").ok()?;
    pane.strip_prefix('%')?.parse::<u32>().ok()?;
    Some((server, pane))
}

pub(super) fn current_pane() -> Option<String> {
    let (server, pane) = location()?;
    Some(format!("{server}:{pane}"))
}

pub(super) fn in_caller_window(candidates: &[&str]) -> Option<usize> {
    // There is nothing tmux can distinguish if all pane IDs are absent or equal.
    let first = candidates.first()?;
    if candidates.iter().all(|pane| pane == first) {
        return None;
    }
    let (server, pane) = location()?;
    // Resolve the window at send time. The editor may be in a background
    // window, so target its pane explicitly.
    let output = Command::new("tmux")
        .args(["list-panes", "-t", &pane, "-F", "#{pid}:#{pane_id}"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let window = String::from_utf8(output.stdout).ok()?;
    let caller = format!("{server}:{pane}");
    if !window.lines().any(|member| member == caller) {
        return None;
    }
    unique_match(&window, candidates)
}

fn unique_match(window: &str, candidates: &[&str]) -> Option<usize> {
    let mut local = candidates
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, pane)| !pane.is_empty() && window.lines().any(|member| member == *pane));
    let (index, _) = local.next()?;
    local.next().is_none().then_some(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_instance_in_the_window_can_win() {
        let window = "17:%1\n17:%2\n17:%3\n";
        // Other server, plain terminal, another window, then the local pane.
        let candidates = ["29:%1", "", "17:%4", "17:%2"];
        assert_eq!(unique_match(window, &candidates), Some(3));
        assert_eq!(unique_match(window, &["29:%1", ""]), None);
        assert_eq!(unique_match(window, &["17:%1", "17:%2"]), None);
        assert_eq!(unique_match(window, &["17:%1", "17:%1"]), None);
    }
}
