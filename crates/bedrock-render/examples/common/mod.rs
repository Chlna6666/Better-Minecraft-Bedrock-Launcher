use log::{LevelFilter, Log, Metadata, Record};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

static LOGGER: OnceLock<ExampleLogger> = OnceLock::new();

pub fn init_logger() {
    let logger = LOGGER.get_or_init(ExampleLogger::from_env);
    if log::set_logger(logger).is_ok() {
        log::set_max_level(logger.max_level());
    }
}

struct ExampleLogger {
    default: LevelFilter,
    directives: Vec<LogDirective>,
    max_level: LevelFilter,
}

struct LogDirective {
    target: String,
    level: LevelFilter,
}

impl ExampleLogger {
    fn from_env() -> Self {
        let mut default = LevelFilter::Warn;
        let mut directives = Vec::new();
        let mut max_level = default;
        if let Ok(value) = std::env::var("RUST_LOG") {
            default = LevelFilter::Off;
            max_level = LevelFilter::Off;
            for token in value
                .split(',')
                .map(str::trim)
                .filter(|token| !token.is_empty())
            {
                if let Some((target, level)) = token.split_once('=') {
                    if let Ok(level) = level.parse::<LevelFilter>() {
                        max_level = max_level.max(level);
                        directives.push(LogDirective {
                            target: target.trim().replace('-', "_"),
                            level,
                        });
                    }
                } else if let Ok(level) = token.parse::<LevelFilter>() {
                    default = level;
                    max_level = max_level.max(level);
                }
            }
        }
        Self {
            default,
            directives,
            max_level,
        }
    }

    const fn max_level(&self) -> LevelFilter {
        self.max_level
    }

    fn level_for(&self, target: &str) -> LevelFilter {
        let mut level = self.default;
        for directive in &self.directives {
            if target.starts_with(&directive.target) {
                level = directive.level;
            }
        }
        level
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
