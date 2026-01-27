use std::fmt;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Trace,
    Info,
    Warn,
}

pub trait LogSink: Send + Sync + 'static {
    fn log(&self, level: Level, target: &'static str, args: fmt::Arguments);
}

static LOG_SINK: OnceLock<Box<dyn LogSink>> = OnceLock::new();

pub fn try_set_log_sink(sink: Box<dyn LogSink>) -> Result<(), Box<dyn LogSink>> {
    LOG_SINK.set(sink)
}

pub fn has_log_sink() -> bool {
    LOG_SINK.get().is_some()
}

pub(crate) fn emit(level: Level, target: &'static str, args: fmt::Arguments) {
    if let Some(sink) = LOG_SINK.get() {
        sink.log(level, target, args);
    }
}
