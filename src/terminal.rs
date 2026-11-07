//! Terminal response helpers and human styling. Structured output never calls styling renderers.
use anyhow::{Context, Result, ensure};
use clap::{
    ColorChoice,
    builder::{
        Styles,
        styling::{AnsiColor, Effects},
    },
};
use serde_json::{Value, json};
use std::{
    io::IsTerminal,
    sync::atomic::{AtomicBool, Ordering},
};

static COLOR: AtomicBool = AtomicBool::new(false);

/// Retain the last native terminal tail rows without changing drafts or cursors.
/// `truncated` reports only whether this already-returned tail was shortened.
pub fn limit_tail(value: &mut Value, rows: usize) -> Result<()> {
    ensure!(rows > 0, "Terminal tail row limit must be positive");
    ensure!(
        value.get("bridgeTail").is_none(),
        "Unsupported terminal.read response: bridgeTail already exists"
    );
    let tail = value
        .pointer_mut("/terminal/tail")
        .and_then(Value::as_array_mut)
        .context("Unsupported terminal.read response: expected terminal.tail array")?;
    let removed = tail.len().saturating_sub(rows);
    tail.drain(..removed);
    let returned = tail.len();
    value["bridgeTail"] = json!({"limit":rows,"returned":returned,"truncated":removed > 0});
    Ok(())
}

pub fn styles() -> Styles {
    Styles::styled()
        .header(AnsiColor::Yellow.on_default().effects(Effects::BOLD))
        .usage(AnsiColor::Yellow.on_default().effects(Effects::BOLD))
        .literal(AnsiColor::Cyan.on_default().effects(Effects::BOLD))
        .placeholder(AnsiColor::Cyan.on_default())
        .error(AnsiColor::Red.on_default().effects(Effects::BOLD))
}

pub fn color_choice() -> ColorChoice {
    // Decide before Clap renders help. An explicit flag wins over environment defaults.
    let args: Vec<_> = std::env::args().collect();
    for (i, arg) in args.iter().enumerate().rev() {
        let value = arg.strip_prefix("--color=").or_else(|| {
            (arg == "--color")
                .then(|| args.get(i + 1).map(String::as_str))
                .flatten()
        });
        if let Some(value) = value {
            match value {
                "always" => return ColorChoice::Always,
                "never" => return ColorChoice::Never,
                _ => {}
            }
        }
    }
    if std::env::var_os("NO_COLOR").is_some() || std::env::var("TERM").as_deref() == Ok("dumb") {
        return ColorChoice::Never;
    }
    match std::env::var("FORCE_COLOR").as_deref() {
        Ok("0") => ColorChoice::Never,
        Ok(s) if !s.is_empty() => ColorChoice::Always,
        _ => ColorChoice::Auto,
    }
}

pub fn configure(choice: ColorChoice) {
    COLOR.store(
        match choice {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => std::io::stdout().is_terminal(),
        },
        Ordering::Relaxed,
    );
}

pub fn paint(value: &str, code: &str) -> String {
    let value: String = value
        .chars()
        .flat_map(|c| {
            if c.is_control() {
                c.escape_default().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect();
    if COLOR.load(Ordering::Relaxed) {
        format!("\x1b[{code}m{value}\x1b[0m")
    } else {
        value
    }
}

pub fn session(value: &Value) -> String {
    let field = |key: &str| value[key].as_str().unwrap_or("");
    format!(
        "{}\n  {} · {} · {}\n  {}\n",
        paint(field("title"), "1;36"),
        paint(field("agent"), "33"),
        paint(value["model"].as_str().unwrap_or("unknown model"), "2"),
        paint(field("cwd"), "2"),
        paint(field("id"), "2")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_limit_preserves_draft_cursors_envelope_and_unknown_fields() {
        let mut value = json!({
            "terminal":{
                "handle":"term_fixture","draft":{"text":"unsent fixture","future":true},
                "tail":[{"row":1},"second",{"row":3,"future":"preserved"}],
                "cursor":42,"futureTerminal":{"kept":true}
            },
            "nextCursor":99,"runtimeId":"fixture","futureEnvelope":[1,2]
        });
        let mut expected = value.clone();
        expected["terminal"]["tail"] = json!(["second",{"row":3,"future":"preserved"}]);
        expected["bridgeTail"] = json!({"limit":2,"returned":2,"truncated":true});
        limit_tail(&mut value, 2).unwrap();
        assert_eq!(value, expected);
    }

    #[test]
    fn tail_limit_reports_short_and_empty_native_tails_without_inventing_truncation() {
        for tail in [json!([]), json!(["fixture"])] {
            let mut value = json!({"terminal":{"tail":tail}});
            limit_tail(&mut value, 7).unwrap();
            assert_eq!(value["terminal"]["tail"], tail);
            assert_eq!(
                value["bridgeTail"],
                json!({"limit":7,"returned":tail.as_array().unwrap().len(),"truncated":false})
            );
        }
    }

    #[test]
    fn tail_limit_rejects_unsupported_shapes_without_mutating_them() {
        for source in [
            json!(null),
            json!([]),
            json!({}),
            json!({"terminal":{"tail":"fixture"}}),
            json!({"tail":[1,2]}),
            json!({"terminal":{"tail":[1,2]},"bridgeTail":{"future":"preserved"}}),
        ] {
            let mut value = source.clone();
            assert!(limit_tail(&mut value, 1).is_err());
            assert_eq!(value, source);
        }
        let mut value = json!({"terminal":{"tail":[1,2]}});
        let original = value.clone();
        assert!(limit_tail(&mut value, 0).is_err());
        assert_eq!(value, original);
    }
}
