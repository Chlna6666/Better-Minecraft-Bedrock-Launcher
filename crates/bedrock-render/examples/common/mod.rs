use log::{LevelFilter, Log, Metadata, Record};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

static LOGGER: OnceLock<ExampleLogger> = OnceLock::new();

pub fn init_logger() {
    let logger = LOGGER.get_or_init(ExampleLogger::new);
    if log::set_logger(logger).is_ok() {
        log::set_max_level(logger.max_level());
    }
}

struct ExampleLogger {
    level: LevelFilter,
}

impl ExampleLogger {
    const fn new() -> Self {
        Self {
            level: LevelFilter::Debug,
        }
    }

    const fn max_level(&self) -> LevelFilter {
        self.level
    }

    const fn level_for(&self, _target: &str) -> LevelFilter {
        self.level
    }
}

impl Log for ExampleLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level().to_level_filter() <= self.level_for(metadata.target())
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs());
        eprintln!(
            "{timestamp} {:>5} {}: {}",
            record.level(),
            record.target(),
            record.args()
        );
    }

    fn flush(&self) {}
}
