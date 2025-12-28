use std::io::IsTerminal;

pub fn use_color() -> bool {
    std::io::stderr().is_terminal()
}

pub fn yellow(text: &str) -> String {
    if use_color() {
        format!("\x1b[1;33m{}\x1b[0m", text)
    } else {
        text.to_string()
    }
}

pub fn magenta(text: &str) -> String {
    if use_color() {
        format!("\x1b[1;35m{}\x1b[0m", text)
    } else {
        text.to_string()
    }
}

pub fn cyan(text: &str) -> String {
    if use_color() {
        format!("\x1b[36m{}\x1b[0m", text)
    } else {
        text.to_string()
    }
}

pub fn bold(text: &str) -> String {
    if use_color() {
        format!("\x1b[1m{}\x1b[0m", text)
    } else {
        text.to_string()
    }
}
