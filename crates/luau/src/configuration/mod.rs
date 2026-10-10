mod client;
mod evaluation;
pub(crate) mod protocol;
mod worker;

pub use client::ConfigurationEvaluator;

pub use evaluation::{
    ConfigurationContext, ConfigurationEntry, ConfigurationInput, ConfigurationKey,
    ConfigurationLimits, ConfigurationResult, ConfigurationValue,
};

pub use worker::run_configuration_worker;
