//! What is left of the context and of the account's usage limits, as bars in the panel the ring
//! under the input opens. The context bar counts up to where the CLI compacts the conversation
//! by itself when the settings say (`autoCompactWindow`), else to the end of the window.

use gpui_kit::component::progress::Progress;
use gpui_kit::*;
use zide_claude::{Defaults, Limits, Transcript, Window as Usage};

use crate::theme;

/// The bar and text colour for a share of 0 to 1 already spent.
pub(super) fn color(used: f64) -> u32 {
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

/// A bar across the panel, with a line above that says what it measures.
fn bar(id: &'static str, label: &str, right: String, used: f64) -> Div {
    let used = used.clamp(0.0, 1.0);
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .justify_between()
                .child(div().text_color(rgb(theme::TEXT)).child(label.to_owned()))
                .child(div().text_color(rgb(color(used))).child(right)),
        )
        .child(
            Progress::new(id)
                .value((used * 100.0) as f32)
                .color(rgb(color(used)))
                .w_full()
                .h(px(4.)),
        )
}

/// What the context bar counts up to: where the conversation compacts by itself if that comes
/// first, else the end of the window. `None` while the window is not known.
pub(super) fn limit(transcript: &Transcript, defaults: &Defaults) -> Option<u64> {
    let window = transcript.context_window()?;
    Some(match defaults.compact_at(transcript.model()) {
        Some(compact) if compact > 0 => compact.min(window),
        _ => window,
    })
}

/// How full the context is, 0 to 1, when its limit is known.
pub(super) fn context_share(transcript: &Transcript, defaults: &Defaults) -> Option<f64> {
    let (usage, limit) = (transcript.usage()?, limit(transcript, defaults)?);
    Some(usage.context_tokens as f64 / limit as f64)
}

/// What the small ring under the input opens: the context window, then the plan's limits.
pub(super) fn report(
    transcript: &Transcript,
    defaults: &Defaults,
    limits: Option<&Limits>,
    now: u64,
) -> Div {
    let label = if defaults.compact_at(transcript.model()).is_some() {
        "Until auto-compact"
    } else {
        "Context window"
    };
    let context = match (transcript.usage(), limit(transcript, defaults)) {
        (Some(usage), Some(window)) => bar(
            "claude-context",
            label,
            format!("{} / {}", tokens(usage.context_tokens), tokens(window)),
            usage.context_tokens as f64 / window as f64,
        ),
        _ => bar("claude-context", "Context window", "0".to_owned(), 0.0),
    };
    let windows: [(&'static str, &str, Option<Usage>); 2] = [
        (
            "claude-5h",
            "5-hour limit",
            limits.and_then(|l| l.five_hour),
        ),
        (
            "claude-7d",
            "Weekly limit",
            limits.and_then(|l| l.seven_day),
        ),
    ];
    let limits = windows.into_iter().filter_map(|(id, label, window)| {
        let window = window?;
        let left = window.resets_at.saturating_sub(now);
        Some(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(bar(
                    id,
                    label,
                    format!("{:.0}%", window.utilization * 100.0),
                    window.utilization,
                ))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(theme::DIM))
                        .child(format!("Resets {}", resets_in(left))),
                ),
        )
    });
    div()
        .flex()
        .flex_col()
        .gap_3()
        .text_sm()
        .child(context)
        .children(limits)
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
