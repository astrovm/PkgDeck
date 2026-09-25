//! Transient terminal feedback on stderr: a spinner while PkgDeck works and
//! permanent one-line results as changes finish. Stdout stays reserved for
//! the command's result, so pipes and `--json` are unaffected.
use std::io::{self, IsTerminal, Write};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use unicode_width::UnicodeWidthChar;

const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

#[derive(Default)]
struct State {
    label: Option<String>,
    detail: String,
    since: Option<Instant>,
    /// Columns covered by the spinner line, erased with spaces so no cursor
    /// escape codes are written when color is off.
    drawn: usize,
    stop: bool,
}

pub struct Live {
    /// Redraw a spinner in place; false when stderr is a pipe or a file.
    animated: bool,
    color: bool,
    width: usize,
    state: Arc<Mutex<State>>,
    thread: Option<JoinHandle<()>>,
}

impl Live {
    pub fn new(color: bool) -> Self {
        let animated =
            io::stderr().is_terminal() && std::env::var("TERM").is_ok_and(|term| term != "dumb");
        Self::with_animation(color, animated)
    }
    /// No spinner and no result lines, for `--json`.
    pub fn off() -> Self {
        Self::with_animation(false, false)
    }
    /// A spinner that animates even when stderr is not a terminal.
    #[cfg(test)]
    pub fn forced(color: bool) -> Self {
        Self::with_animation(color, true)
    }
    fn with_animation(color: bool, animated: bool) -> Self {
        let width = rustix::termios::tcgetwinsize(io::stderr())
            .map_or(80, |size| usize::from(size.ws_col))
            .clamp(24, 200);
        let state = Arc::new(Mutex::new(State::default()));
        let thread = animated.then(|| {
            let state = Arc::clone(&state);
            std::thread::spawn(move || {
                let mut frame = 0;
                loop {
                    std::thread::sleep(Duration::from_millis(80));
                    let mut state = state.lock().unwrap();
                    if state.stop {
                        break;
                    }
                    if state.label.is_some() {
                        frame = (frame + 1) % FRAMES.len();
                        draw(&mut state, frame, width, color);
                    }
                }
            })
        });
        Self {
            animated,
            color,
            width,
            state,
            thread,
        }
    }
    pub fn color(&self) -> bool {
        self.color
    }
    /// True when stderr is a terminal and the spinner is shown.
    pub fn animated(&self) -> bool {
        self.animated
    }
    /// Show what PkgDeck is doing now. Replaces the previous status.
    pub fn status(&self, label: impl Into<String>) {
        if !self.animated {
            return;
        }
        let mut state = self.state.lock().unwrap();
        state.label = Some(label.into());
        state.detail.clear();
        state.since = Some(Instant::now());
        draw(&mut state, 0, self.width, self.color);
    }
    /// A short secondary note next to the status, such as the latest manager output.
    pub fn detail(&self, text: &str) {
        if !self.animated {
            return;
        }
        let line = text.lines().rev().find(|line| !line.trim().is_empty());
        let mut state = self.state.lock().unwrap();
        state.detail = line
            .map(|line| crate::presentation::clean(line.trim()))
            .unwrap_or_default();
    }
    /// Remove the spinner line, for example before a prompt.
    pub fn clear(&self) {
        let mut state = self.state.lock().unwrap();
        state.label = None;
        erase(&mut state);
    }
    /// Print a permanent line above the spinner.
    pub fn line(&self, text: &str) {
        let mut state = self.state.lock().unwrap();
        erase(&mut state);
        let mut err = io::stderr().lock();
        let _ = writeln!(err, "{text}");
        let _ = err.flush();
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        {
            let mut state = self.state.lock().unwrap();
            state.stop = true;
            erase(&mut state);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn erase(state: &mut State) {
    if state.drawn > 0 {
        let mut err = io::stderr().lock();
        let _ = write!(err, "\r{}\r", " ".repeat(state.drawn));
        let _ = err.flush();
        state.drawn = 0;
    }
}

fn draw(state: &mut State, frame: usize, width: usize, color: bool) {
    let Some(label) = &state.label else {
        return;
    };
    let seconds = state.since.map_or(0, |since| since.elapsed().as_secs());
    let timer = if seconds >= 2 {
        format!(" {seconds}s")
    } else {
        String::new()
    };
    // Keep the line one column short of the edge so the terminal never wraps it.
    let room = width.saturating_sub(3 + timer.len() + 1);
    let label = fit(label, room);
    let detail_room = room.saturating_sub(display_width(&label) + 3);
    let detail = if state.detail.is_empty() || detail_room < 8 {
        String::new()
    } else {
        fit(&state.detail, detail_room)
    };
    let (spin, dim, reset) = if color {
        ("\x1b[36m", "\x1b[2m", "\x1b[0m")
    } else {
        ("", "", "")
    };
    let mut line = format!("\r{spin}{}{reset} {label}", FRAMES[frame]);
    let mut columns = 2 + display_width(&label);
    if !detail.is_empty() {
        line.push_str(&format!("{dim} · {detail}{reset}"));
        columns += 3 + display_width(&detail);
    }
    if !timer.is_empty() {
        line.push_str(&format!("{dim}{timer}{reset}"));
        columns += timer.len();
    }
    line.push_str(&" ".repeat(state.drawn.saturating_sub(columns)));
    let mut err = io::stderr().lock();
    let _ = write!(err, "{line}");
    let _ = err.flush();
    state.drawn = state.drawn.max(columns);
}

fn display_width(text: &str) -> usize {
    text.chars().filter_map(UnicodeWidthChar::width).sum()
}

fn fit(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return text.into();
    }
    let mut output = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        used += w;
        output.push(c);
    }
    output.push('…');
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn spinner_draws_status_detail_and_timer_then_clears() {
        for color in [false, true] {
            let live = Live::forced(color);
            assert!(live.animated() && live.color() == color);
            live.status("Installing a package with a long name");
            live.detail("line one\n\nUnpacking tool\n");
            assert_eq!(live.state.lock().unwrap().detail, "Unpacking tool");
            std::thread::sleep(Duration::from_millis(200));
            live.line("✓ Install tool");
            let mut state = live.state.lock().unwrap();
            state.since = Some(Instant::now() - Duration::from_secs(3));
            draw(&mut state, 1, 60, color);
            assert!(state.drawn > 0);
            // Too narrow for the detail: only the label is drawn.
            draw(&mut state, 2, 24, color);
            drop(state);
            live.clear();
            assert_eq!(live.state.lock().unwrap().drawn, 0);
        }
        let quiet = Live::off();
        assert!(!quiet.animated());
        quiet.status("ignored");
        quiet.detail("ignored");
        assert!(quiet.state.lock().unwrap().label.is_none());
    }
    #[test]
    fn fit_truncates_by_display_width() {
        assert_eq!(fit("abc", 5), "abc");
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit("工具工具", 5), "工具…");
    }
}
