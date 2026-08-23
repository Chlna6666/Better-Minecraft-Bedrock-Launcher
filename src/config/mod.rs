pub mod agreement;
pub mod config;
mod defaults;
pub mod onboarding;
mod storage;

#[cfg(target_os = "windows")]
pub mod uwp_safety;

#[cfg(test)]
mod test;
