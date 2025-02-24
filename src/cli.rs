use console::style;

#[derive(Debug, Clone, Copy)]
pub enum LogLevel {
    Info,
    Warning,
    Error,
}

pub fn log(level: LogLevel, message: &str) {
    eprintln!(
        "{} {}",
        match level {
            LogLevel::Info => style(""),
            LogLevel::Warning => style("warning:").yellow().bold(),
            LogLevel::Error => style("error:").red().bold(),
        },
        style(message).bold(),
    );
}

pub fn info(message: &str) {
    eprintln!("{}", style(message).bold(),);
}

pub fn warning(message: &str) {
    log(LogLevel::Warning, message);
}

pub fn error(message: &str) {
    log(LogLevel::Error, message);
}
