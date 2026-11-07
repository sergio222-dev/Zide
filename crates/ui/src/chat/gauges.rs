//! What is left of the context and of the account's usage limits, as small bars beside the
//! dropdowns under the input. The CLI does not say when it compacts the conversation by
//! itself, so the context bar shows how full the window is, not the distance to that point.

use gpui_kit::component::progress::Progress;
use gpui_kit::*;
use zide_claude::{Limits, Transcript, Window as Usage};

use crate::theme;

/// The bar and text colour for a share of 0 to 1 already spent.
fn color(used: f64) -> u32 {
    if used >= 0.9 {
        theme::ERROR
    } else if used >= 0.7 {
        theme::WORKING
    } else {
        theme::OK
    }
}

/// "850k", "1.2M".
fn tokens(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else {
        format!("{}k", count / 1000)
    }
}

/// When a limit starts over: "in 2h 5m", "in 3d 4h", "now".
fn resets_in(seconds: u64) -> String {
    let (days, hours, minutes) = (seconds / 86_400, seconds / 3600 % 24, seconds / 60 % 60);
    match (days, hours, minutes) {
        (0, 0, 0) => "now".to_owned(),
        (0, 0, m) => format!("in {m}m"),
        (0, h, m) => format!("in {h}h {m}m"),
        (d, h, _) => format!("in {d}d {h}h"),
    }
}

fn gauge(id: &'static str, label: &str, used: f64, text: String) -> Div {
    let used = used.clamp(0.0, 1.0);
    div()
        .flex()
        .items_center()
        .gap_1p5()
        .child(div().text_color(rgb(theme::DIM)).child(label.to_owned()))
        .child(
            Progress::new(id)
                .value((used * 100.0) as f32)
                .color(rgb(color(used)))
                .w(px(64.))
                .h(px(6.)),
        )
        .child(div().text_color(rgb(color(used))).child(text))
}

/// The context bar, when the window is known, and a bar per usage window.
pub(super) fn view(transcript: &Transcript, limits: Option<&Limits>, now: u64) -> Div {
    let mut row = div().flex().items_center().gap_4().text_xs();
    if let (Some(usage), Some(window)) = (transcript.usage(), transcript.context_window()) {
        let used = usage.context_tokens as f64 / window as f64;
        row = row.child(gauge(
            "claude-context",
            "Context",
            used,
            format!("{} / {}", tokens(usage.context_tokens), tokens(window)),
        ));
    }
    let windows: [(&'static str, &str, Option<Usage>); 2] = [
        ("claude-5h", "5h", limits.and_then(|l| l.five_hour)),
        ("claude-7d", "7d", limits.and_then(|l| l.seven_day)),
    ];
    for (id, label, window) in windows {
        if let Some(window) = window {
            let left = window.resets_at.saturating_sub(now);
            row = row.child(gauge(
                id,
                label,
                window.utilization,
                format!("{:.0}% · {}", window.utilization * 100.0, resets_in(left)),
            ));
        }
    }
    row
}

#[cfg(test)]
mod tests {
    use super::{color, resets_in, tokens};
    use crate::theme;

    #[test]
    fn colors_warn_as_the_share_grows() {
        assert_eq!(color(0.2), theme::OK);
        assert_eq!(color(0.7), theme::WORKING);
        assert_eq!(color(0.95), theme::ERROR);
    }

    #[test]
    fn tokens_read_short() {
        assert_eq!(tokens(850_400), "850k");
        assert_eq!(tokens(1_300_000), "1.3M");
    }

    #[test]
    fn the_reset_names_two_units() {
        assert_eq!(resets_in(0), "now");
        assert_eq!(resets_in(5 * 60 + 9), "in 5m");
        assert_eq!(resets_in(2 * 3600 + 5 * 60), "in 2h 5m");
        assert_eq!(resets_in(3 * 86_400 + 4 * 3600), "in 3d 4h");
    }
}
