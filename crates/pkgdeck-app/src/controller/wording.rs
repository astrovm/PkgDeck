//! Errors and tool output in people's words: one plain sentence, naming
//! the tool when it is known.
use super::*;

/// Capitalize and end with a period, so every message reads as a sentence.
pub(super) fn sentence(text: &str) -> String {
    let text = text.trim().trim_end_matches([':', ';', ',']).trim_end();
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut out: String = first.to_uppercase().chain(chars).collect();
    if !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}
/// Drop Rust debug formatting that leaks from lower layers, such as
/// "(Some(1))" or "Some(\"x\")", keeping the meaningful value.
pub(super) fn strip_debug(text: &str) -> String {
    let mut out = text.replace("(None)", "").replace(" None:", ":");
    while let Some(start) = out.find("Some(") {
        let inner = start + "Some(".len();
        let Some(end) = out[inner..].find(')').map(|at| inner + at) else {
            out.replace_range(start..inner, "");
            break;
        };
        let value = out[inner..end].trim_matches('"').to_owned();
        out.replace_range(start..=end, &value);
    }
    out
}
/// The most output shown under a failure. The end matters most: it is where
/// tools print what went wrong.
pub(super) const FAILURE_OUTPUT_LIMIT: usize = 6000;
/// Everything a failed command printed, for the failure's details, so the
/// reason is readable without running it again in a terminal.
pub(super) fn raw_failure_output(error: &EngineError) -> Option<String> {
    let EngineError::Execution(pkgdeck_core::process::ExecutionError::Failed(result)) = error
    else {
        return None;
    };
    let stderr = String::from_utf8_lossy(&result.stderr);
    let stdout = String::from_utf8_lossy(&result.stdout);
    let text = [stdout.trim(), stderr.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        return None;
    }
    let mut start = text.len().saturating_sub(FAILURE_OUTPUT_LIMIT);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    Some(if start > 0 {
        format!("…{}", &text[start..])
    } else {
        text
    })
}
/// Whether a failed OpenSSL build says its headers or pkg-config are
/// missing, the only case installing them fixes.
pub(super) fn missing_openssl_files(output: &str) -> bool {
    let output = output.to_ascii_lowercase();
    [
        "pkg-config",
        "pkg_config",
        "opensslconf.h",
        "could not find openssl",
        "could not find directory of openssl",
    ]
    .iter()
    .any(|marker| output.contains(marker))
}
/// The first line of tool output worth showing: an error line if there is
/// one, else the first non-empty line. Prefixes such as "error:" and "E:"
/// are removed because the sentence around it already says it failed.
pub(super) fn meaningful_line(output: &str) -> Option<String> {
    const PREFIXES: &[&str] = &["error:", "Error:", "ERROR:", "E:", "fatal:", "FATAL:"];
    let lines: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.starts_with("WARNING: apt does not have a stable CLI"))
        .collect();
    let line = lines
        .iter()
        .find(|line| PREFIXES.iter().any(|prefix| line.starts_with(prefix)))
        .or_else(|| lines.first())?;
    let mut line = *line;
    for prefix in PREFIXES {
        if let Some(rest) = line.strip_prefix(prefix) {
            line = rest.trim_start();
        }
    }
    // Well-known messages people hit often, said plainly.
    if line.contains("rustup could not choose a version")
        || line.contains("no default toolchain")
        || line.contains("no default is configured")
    {
        return Some("rustup has no default toolchain".into());
    }
    // Cargo builds a dependency's C bindings while installing. Say which
    // one failed, and the usual reason for the one people hit most.
    if let Some(krate) = line
        .strip_prefix("failed to run custom build command for `")
        .and_then(|rest| rest.split('`').next())
    {
        return Some(
            if krate.starts_with("openssl-sys ") && missing_openssl_files(output) {
                format!("building `{krate}` failed. It needs OpenSSL's development files (libssl-dev or openssl-devel) and pkg-config")
            } else {
                format!(
                    "building `{krate}` failed. Run the update in a terminal to read the build log"
                )
            },
        );
    }
    // Flatpak names the remote and the URL; people need the remote and why.
    if let Some(rest) = line.strip_prefix("Unable to load summary from remote ") {
        let remote = rest.split(':').next().unwrap_or_default();
        let reason = rest.rsplit(": ").next().unwrap_or_default();
        let reason = reason
            .split_once("] ")
            .filter(|(code, _)| code.starts_with('['))
            .map_or(reason, |(_, reason)| reason);
        return Some(format!("it can't reach the remote {remote} ({reason})"));
    }
    Some(line.trim_end_matches(['.', ':', ';']).to_owned())
}
pub(super) const ROOT_MESSAGE: &str =
    "PkgDeck can't make changes while running as root. Start it as your normal user.";
/// Whether engine text is the refusal to make changes as root, in the
/// core's current wording or an older one.
pub(super) fn refuses_root(text: &str) -> bool {
    text.contains("unprivileged user")
        || text.contains("running as root")
        || text.contains("runs as root")
}
/// One plain sentence for text that came from the engine, whatever its
/// wording: internal prefixes and debug formatting are removed, and the
/// tool's first meaningful output line is kept.
pub(super) fn plain_text(raw: &str, backend: Option<&str>) -> String {
    if refuses_root(raw) {
        return ROOT_MESSAGE.into();
    }
    let tool = backend.map(source_display_name);
    let failed = |output: &str| match (meaningful_line(output), &tool) {
        (Some(line), Some(tool)) => format!("{tool} couldn't run: {}", sentence_tail(&line)),
        (Some(line), None) => sentence(&line),
        (None, Some(tool)) => format!("{tool} reported an error without details."),
        (None, None) => "The package manager reported an error without details.".into(),
    };
    if let Some(at) = raw.find("host command failed") {
        let rest = &raw[at..];
        return failed(rest.find("):").map_or("", |end| &rest[end + 2..]));
    }
    // Core's own wording: "the package manager exited with code N: reason".
    if let Some(rest) = raw.trim_start().strip_prefix("the package manager ") {
        return failed(rest.split_once(": ").map_or("", |(_, reason)| reason));
    }
    let cleaned = strip_debug(raw);
    let line = meaningful_line(&cleaned).unwrap_or_default();
    if line.is_empty() {
        return "Something went wrong.".into();
    }
    sentence(&line)
}
/// The part after "X couldn't run:" keeps its own case (tool names such as
/// rustup are lowercase) and ends with a period.
pub(super) fn sentence_tail(line: &str) -> String {
    let mut line = line.trim().to_owned();
    if !line.ends_with(['.', '!', '?']) {
        line.push('.');
    }
    line
}
/// Lowercase a sentence's first letter to continue another sentence,
/// unless it starts an acronym or a name such as "APT".
pub(super) fn lower_first(text: &str) -> String {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(first), Some(second)) if first.is_uppercase() && second.is_lowercase() => {
            first.to_lowercase().chain(text.chars().skip(1)).collect()
        }
        _ => text.to_owned(),
    }
}
/// The macOS inventory lists every app it can read and reports each folder
/// or bundle it had to skip. That is a partial result, not a failed source.
pub(super) fn skipped_app_entry(error: &EngineError) -> Option<(&str, &str)> {
    let EngineError::InvalidResponse { backend, reason } = error else {
        return None;
    };
    if backend != "macos-apps" {
        return None;
    }
    reason
        .strip_prefix("skipped folder ")
        .or_else(|| reason.strip_prefix("skipped entry in "))
        .or_else(|| reason.strip_prefix("skipped entry "))?
        .rsplit_once(": ")
}
/// A cask script ran sudo and got no password: the dialog was cancelled,
/// the run was automatic, or this build has no dialog to show.
pub(super) fn cask_needed_password(error: &EngineError, backend: Option<&str>) -> bool {
    matches!(
        error,
        EngineError::Execution(pkgdeck_core::process::ExecutionError::Failed(result))
            if matches!(backend, Some("homebrew" | "homebrew-cask"))
                && String::from_utf8_lossy(&result.stderr).contains("/usr/bin/sudo")
    )
}
/// Why a change didn't finish, as Activity keeps it: [`plain_error`]
/// without anything a tool printed, which Activity never stores.
pub(super) fn kept_reason(error: &EngineError, backend: &str) -> String {
    use pkgdeck_core::process::ExecutionError as E;
    let tool = source_display_name(backend);
    match error {
        EngineError::Execution(E::Failed(_)) => format!("{tool} stopped with an error."),
        EngineError::InvalidResponse { .. } => format!("{tool} reported a problem."),
        error => plain_error(error, Some(backend), false),
    }
}
/// A plain explanation of an engine error, naming the tool when known.
pub(super) fn plain_error(error: &EngineError, backend: Option<&str>, sudo: bool) -> String {
    use pkgdeck_core::process::ExecutionError as E;
    if let Some((path, why)) = skipped_app_entry(error) {
        let why = why.split(" (os error ").next().unwrap_or(why);
        return format!(
            "Couldn't read {path} ({}). The other apps are still listed.",
            lower_first(why)
        );
    }
    match error {
        // mas asks for the Mac password through sudo, which needs a terminal.
        EngineError::Execution(E::AuthorizationDenied) if backend == Some("mas") => "App Store updates need your Mac password, which mas can only ask for in a terminal. PkgDeck opened the App Store's Updates page so you can update there, or run `pkd upgrade --from mas` in Terminal.".into(),
        EngineError::Execution(E::AuthorizationDenied) if sudo => "PkgDeck couldn't get administrator access. The sudo option needs a recent sudo login in the same terminal.".into(),
        EngineError::Execution(E::AuthorizationDenied) => "PkgDeck couldn't get administrator access. Make sure your desktop's password prompt is running, then try again.".into(),
        EngineError::Execution(E::AuthorizationCancelled) => "The password prompt was closed. Nothing was changed.".into(),
        EngineError::Execution(E::LockBusy) => "Another package manager is running. Wait for it to finish, then try again.".into(),
        EngineError::Execution(E::Interrupted) => "The package manager was interrupted. Check its state before trying again.".into(),
        EngineError::Execution(E::TimedOut) => "The package manager took too long to answer. Try again.".into(),
        EngineError::Cancelled | EngineError::Execution(E::Cancelled) => "Cancelled.".into(),
        error if cask_needed_password(error, backend) => {
            "Homebrew needed your administrator password to finish this change and didn't get one. Update it again from Updates and enter your password when asked.".into()
        }
        EngineError::Execution(E::Failed(result)) => {
            let output = String::from_utf8_lossy(&result.stderr);
            let output = if output.trim().is_empty() {
                String::from_utf8_lossy(&result.stdout).into_owned()
            } else {
                output.into_owned()
            };
            match (meaningful_line(&output), backend.map(source_display_name)) {
                (Some(line), Some(tool)) => format!("{tool} couldn't run: {}", sentence_tail(&line)),
                (Some(line), None) => sentence(&line),
                (None, Some(tool)) => format!("{tool} reported an error without details."),
                (None, None) => "The package manager reported an error.".into(),
            }
        }
        EngineError::Unavailable { backend, reason } => {
            if refuses_root(reason) {
                return ROOT_MESSAGE.into();
            }
            format!(
                "{} isn't available: {}",
                source_display_name(backend),
                lower_first(&plain_text(reason, None))
            )
        }
        EngineError::InvalidResponse { backend: source, reason }
            if matches!(source.as_str(), "open" | "check") || reason.contains("host command failed") =>
        {
            plain_text(reason, Some(source.as_str()).filter(|s| !matches!(*s, "open" | "check")))
        }
        EngineError::InvalidResponse { backend: source, reason } => {
            if refuses_root(reason) {
                return ROOT_MESSAGE.into();
            }
            match meaningful_line(&strip_debug(reason)) {
                Some(line) => format!(
                    "{} reported a problem: {}",
                    source_display_name(source),
                    sentence_tail(&line)
                ),
                None => format!("{} reported a problem.", source_display_name(source)),
            }
        }
        // PkgDeck's own checks already say it in a sentence.
        EngineError::Refused { refusal, .. } => refusal.to_string(),
        EngineError::Unsupported { backend, .. } => format!(
            "{} can't do this.",
            source_display_name(backend)
        ),
        EngineError::NotFound => "No package matches the selection.".into(),
        other => plain_text(&other.to_string(), backend),
    }
}
